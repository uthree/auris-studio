//! Permission decisions and controls owned by the desktop session.
use super::*;
use auris_session::agent_policy::{Decision, Mode, OPERATIONS, Operation};

const MODES: [Mode; 5] = [
    Mode::ReadOnly,
    Mode::Edit,
    Mode::Plan,
    Mode::AutoReview,
    Mode::Bypass,
];

#[derive(Default)]
pub(super) struct Controls {
    pub(super) pending: Option<Pending>,
    pub(super) review: Option<(u64, auris_agent::approval_review::ReviewJob)>,
    pub(super) permits: Vec<(serde_json::Value, u64)>,
    /// What the user typed into this conversation in this window, oldest first.
    ///
    /// Automatic review judges an operation against these and nothing else. A restored transcript
    /// is a file that travels with the project, so the "requests" in it are whatever its author
    /// wrote and cannot stand for the person at the keyboard.
    pub(super) typed_requests: Vec<String>,
    pub(super) rules_open: bool,
    pub(super) compacting: bool,
    pub(super) mode_menu: bool,
    pub(super) effort_menu: bool,
    pub(super) mode_focus: Option<gpui::FocusHandle>,
    pub(super) effort_focus: Option<gpui::FocusHandle>,
    pub(super) slash_selected: usize,
}

pub(super) struct SlashMatch {
    pub(super) fill: String,
    pub(super) label: String,
    pub(super) description: Key,
}

pub(super) struct Pending {
    id: u64,
    operation: Operation,
    args: serde_json::Value,
    revision: u64,
}

fn completed_review_operations(entries: &[ChatEntry]) -> Vec<serde_json::Value> {
    let mut completed = entries
        .iter()
        .rev()
        .filter_map(|entry| {
            if let ChatEntry::Tool {
                name,
                args,
                ok: true,
                detail,
                ..
            } = entry
                && matches!(name.as_str(), "cli_command" | "cli_file_change")
                && !detail.is_empty()
            {
                serde_json::from_str::<serde_json::Value>(args).ok().map(|arguments|
                serde_json::json!({"operation":name,"arguments":arguments,"result":detail}))
            } else {
                None
            }
        })
        .take(8)
        .collect::<Vec<_>>();
    completed.reverse();
    completed
}

#[derive(Clone, Copy)]
pub(super) enum Approval {
    Once,
    Always,
    Deny,
    Reviewed,
}

/// Whether a mode takes the person at the keyboard out of the loop, and so has to say so on
/// screen for as long as it is chosen rather than once when it is picked.
pub(super) fn mode_warns(mode: Mode) -> bool {
    matches!(mode, Mode::Bypass)
}

/// What an operation awaiting approval would do, in words, when its kind says so with
/// certainty. `None` leaves the card to the operation's own name and arguments.
pub(super) fn approval_summary(operation: &Operation) -> Option<Key> {
    match operation.name.as_str() {
        "cli_command" => Some(Key::AgentApprovalRunCommand),
        "cli_file_change" => Some(Key::AgentApprovalChangeFiles),
        "cli_file_read" => Some(Key::AgentApprovalReadOutside),
        "search_internet" => Some(Key::AgentApprovalInternet),
        _ if operation.mutating && operation.confirm => Some(Key::AgentApprovalReplaces),
        _ if operation.mutating => Some(Key::AgentApprovalEdits),
        _ => None,
    }
}

fn mode_key(mode: Mode) -> Key {
    match mode {
        Mode::ReadOnly => Key::AgentReadOnly,
        Mode::Edit => Key::AgentEditMode,
        Mode::AutoReview => Key::AgentAutoReview,
        Mode::Plan => Key::AgentPlanMode,
        Mode::Bypass => Key::AgentBypassMode,
    }
}

pub(crate) fn effort_key(effort: ReasoningEffort) -> Key {
    match effort {
        ReasoningEffort::Default => Key::AgentEffortDefault,
        ReasoningEffort::None => Key::AgentEffortNone,
        ReasoningEffort::Minimal => Key::AgentEffortMinimal,
        ReasoningEffort::Low => Key::AgentEffortLow,
        ReasoningEffort::Medium => Key::AgentEffortMedium,
        ReasoningEffort::High => Key::AgentEffortHigh,
        ReasoningEffort::Xhigh => Key::AgentEffortXhigh,
        ReasoningEffort::Max => Key::AgentEffortMax,
    }
}

pub(super) fn effort_choices(openai: bool) -> &'static [ReasoningEffort] {
    if openai {
        &[
            ReasoningEffort::Default,
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
        ]
    } else {
        &[
            ReasoningEffort::Default,
            ReasoningEffort::None,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Max,
        ]
    }
}

const SLASH_COMMANDS: &[(&str, bool, Key)] = &[
    ("/mode", true, Key::AgentSlashMode),
    ("/effort", true, Key::AgentSlashEffort),
    ("/permissions", false, Key::AgentSlashPermissions),
    ("/allow", true, Key::AgentSlashAllow),
    ("/deny", true, Key::AgentSlashDeny),
    ("/default", true, Key::AgentSlashDefault),
    ("/compact", false, Key::AgentSlashCompact),
    ("/read-only", false, Key::AgentSlashMode),
    ("/edit", false, Key::AgentSlashMode),
    ("/plan", false, Key::AgentSlashMode),
    ("/auto-review", false, Key::AgentSlashMode),
    ("/bypass", false, Key::AgentSlashMode),
];

/// Whether the text is handled locally before model and project-reload checks.
pub(super) fn is_control_command(text: &str) -> bool {
    let command = text.split_once(' ').map_or(text, |(command, _)| command);
    SLASH_COMMANDS.iter().any(|(name, _, _)| *name == command)
}

pub(super) fn slash_matches(value: &str) -> Vec<SlashMatch> {
    if !value.starts_with('/') || value.contains('\n') {
        return Vec::new();
    }
    let Some((command, argument)) = value.split_once(' ') else {
        return SLASH_COMMANDS
            .iter()
            .copied()
            .filter(|(name, _, _)| name.starts_with(value))
            .map(|(name, takes_argument, description)| SlashMatch {
                fill: format!("{name}{}", if takes_argument { " " } else { "" }),
                label: name.to_string(),
                description,
            })
            .collect();
    };
    let argument = argument.trim_start();
    let (values, description): (Vec<&str>, Key) = match command {
        "/mode" => (
            vec!["read-only", "edit", "plan", "auto_review", "bypass"],
            Key::AgentSlashMode,
        ),
        "/effort" => (
            vec![
                "default", "none", "minimal", "low", "medium", "high", "xhigh", "max",
            ],
            Key::AgentSlashEffort,
        ),
        "/allow" => (
            std::iter::once("*")
                .chain(std::iter::once("edit_project.*"))
                .chain(OPERATIONS.iter().copied())
                .collect(),
            Key::AgentSlashAllow,
        ),
        "/deny" => (
            std::iter::once("*")
                .chain(std::iter::once("edit_project.*"))
                .chain(OPERATIONS.iter().copied())
                .collect(),
            Key::AgentSlashDeny,
        ),
        "/default" => (
            std::iter::once("*")
                .chain(std::iter::once("edit_project.*"))
                .chain(OPERATIONS.iter().copied())
                .collect(),
            Key::AgentSlashDefault,
        ),
        _ => return Vec::new(),
    };
    values
        .into_iter()
        .filter(|candidate| candidate.starts_with(argument))
        .map(|candidate| SlashMatch {
            fill: format!("{command} {candidate}"),
            label: candidate.to_string(),
            description,
        })
        .collect()
}

/// The requests an automatic reviewer may weigh an operation against: the newest ones the user
/// typed in this window, as many as fit the review's evidence budget.
fn review_requests(typed: &[String]) -> Vec<String> {
    const BUDGET: usize = 32 * 1024;
    let mut used = 0usize;
    let mut newest: Vec<String> = typed
        .iter()
        .rev()
        .take_while(|request| {
            used = used.saturating_add(request.len());
            used <= BUDGET
        })
        .cloned()
        .collect();
    newest.reverse();
    newest
}

impl AurisApp {
    /// Common wire-event route used by polling and the headless desktop harness.
    pub(super) fn route_agent_control_event(&mut self, event: &AgentEvent) -> bool {
        self.observe_agent_trace_event(event);
        if !self.agent_chat.interrupting
            && let AgentEvent::Permission { id, tool, args, .. } = event
        {
            self.agent_permission(*id, tool.clone(), args.clone());
            return true;
        }
        false
    }
    /// Observe the same parsed events used by the desktop polling path before policy/execution.
    pub(super) fn observe_agent_trace_event(&self, event: &AgentEvent) {
        match event {
            AgentEvent::Trace { call_id, tool, ok } => {
                if let Some(ok) = ok {
                    let _ = self.session.record_execution_outcome(
                        call_id,
                        tool,
                        if *ok { "success" } else { "failed" },
                    );
                } else {
                    let _ = self.session.record_execution_call(call_id, tool);
                }
            }
            AgentEvent::Call { call_id, tool, .. } => {
                let _ = self.session.record_execution_call(call_id, tool);
            }
            AgentEvent::Result {
                call_id, tool, ok, ..
            } => {
                let _ = self.session.record_execution_outcome(
                    call_id,
                    tool,
                    if *ok { "success" } else { "failed" },
                );
            }
            AgentEvent::Permission {
                id,
                origin,
                tool,
                args,
            } => {
                let operation = Operation::parse(tool, args)
                    .map(|operation| operation.name)
                    .unwrap_or_else(|_| tool.clone());
                let _ = self
                    .session
                    .record_permission_request(*id, origin.clone(), &operation);
            }
            AgentEvent::Ended | AgentEvent::Error { .. } => self.session.cancel_execution_calls(),
            _ => {}
        }
    }

    /// Opens or closes the permission rules and reveals their start when they become visible.
    fn set_agent_rules_open(&mut self, open: bool) {
        self.agent_chat.controls.rules_open = open;
        if open {
            // The rules are the first child of the transcript scroller. A retained tail offset
            // otherwise opens them above the viewport, making the button appear to do nothing.
            self.agent_chat
                .scroll
                .set_offset(gpui::point(px(0.0), px(0.0)));
        }
    }

    fn save_agent_policy_with<E>(
        &mut self,
        previous: AgentPreferences,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<(), E> {
        if let Err(error) = save(&self.settings) {
            self.settings.agent = previous.clone();
            self.agent_chat.policy = previous.policy;
            self.agent_chat.auto_compact_percent = previous.auto_compact_percent;
            return Err(error);
        }
        self.agent_chat.policy = self.settings.agent.policy.clone();
        self.agent_chat.auto_compact_percent = self.settings.agent.auto_compact_percent;
        Ok(())
    }

    fn save_agent_policy(&mut self, previous: AgentPreferences) {
        if let Err(error) = self.save_agent_policy_with(previous, self.settings_saver()) {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
        }
    }

    pub(super) fn agent_mode(&mut self, mode: Mode) {
        if self.settings.agent.policy.mode == mode {
            self.focus_agent_field(AgentField::Chat);
            return;
        }
        if self.agent_operation_busy() {
            return;
        }
        // Changing permissions invalidates previously approved operations.
        if self.agent_chat.controls.pending.is_some() {
            self.agent_approval(Approval::Deny);
        }
        self.agent_chat.controls.permits.clear();
        let previous = self.settings.agent.clone();
        self.settings.agent.policy.mode = mode;
        self.agent_chat.controls.mode_menu = false;
        self.save_agent_policy(previous);
        self.focus_agent_field(AgentField::Chat);
    }

    pub(super) fn agent_effort(&mut self, effort: ReasoningEffort) {
        if self.agent_chat.backend == "opencode" && effort != ReasoningEffort::Default {
            self.agent_chat
                .push_entry(ChatEntry::Note(Key::AgentOpenCodeVariantHelp));
            self.focus_agent_field(AgentField::Chat);
            return;
        }
        let previous = (self.agent_chat.effort, self.agent_chat.thinking);
        self.agent_chat.effort = effort;
        self.agent_chat.thinking = None;
        self.agent_chat.controls.effort_menu = false;
        if let Err(error) = self.agent_write_through_with(self.settings_saver()) {
            (self.agent_chat.effort, self.agent_chat.thinking) = previous;
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
        }
        self.focus_agent_field(AgentField::Chat);
    }

    pub(super) fn agent_fast_mode(&mut self) {
        if let Err(error) = self.agent_fast_mode_with(|settings| settings.save()) {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
        }
        self.focus_agent_field(AgentField::Chat);
    }

    pub(super) fn agent_fast_mode_with<E>(
        &mut self,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) -> Result<(), E> {
        if self.agent_operation_busy() || !self.agent_chat.preferences().supports_fast_mode() {
            return Ok(());
        }
        self.agent_chat.fast_mode = !self.agent_chat.fast_mode;
        if let Err(error) = self.agent_write_through_with(save) {
            self.agent_chat.fast_mode = !self.agent_chat.fast_mode;
            return Err(error);
        }
        Ok(())
    }

    fn permission_result(&mut self, id: u64, ok: bool, reason: &str) {
        let _ = self
            .session
            .record_permission_outcome(id, if ok { "allowed" } else { "denied" });
        let wire =
            serde_json::json!({"event":"permission_result", "id":id, "ok":ok, "reason":reason});
        if let Some(link) = self.agent_chat.link.as_mut()
            && let Err(error) = link.send(&wire.to_string())
        {
            let message = error.to_string();
            self.agent_chat.finish_open_tools("failed", &message);
            self.agent_chat.push_entry(ChatEntry::Error(message));
            self.agent_chat.link = None;
            self.agent_chat.busy = false;
        }
    }

    pub(super) fn agent_permission(&mut self, id: u64, tool: String, args: serde_json::Value) {
        if self.agent_chat.bound_project.as_deref() != self.session.path() {
            self.permission_result(
                id,
                false,
                "The open document changed. Start a new conversation.",
            );
            return;
        }
        let operation = match Operation::parse(&tool, &args) {
            Ok(operation) => operation,
            Err(error) => {
                self.permission_result(id, false, &error);
                return;
            }
        };
        match self.settings.agent.policy.decide(&operation) {
            Decision::Allow => self.permission_result(id, true, "Allowed by current permissions"),
            Decision::Deny(reason) => self.permission_result(id, false, &reason),
            decision @ (Decision::Ask | Decision::Review) => {
                if self.agent_chat.controls.pending.is_some() {
                    self.permission_result(id, false, "Another operation is awaiting confirmation");
                    return;
                }
                self.agent_chat.controls.pending = Some(Pending {
                    id,
                    operation,
                    args,
                    revision: self.session.revision(),
                });
                if decision == Decision::Review {
                    let requests = review_requests(&self.agent_chat.controls.typed_requests);
                    let pending = self.agent_chat.controls.pending.as_ref().unwrap();
                    let completed = completed_review_operations(&self.agent_chat.entries);
                    let result = auris_agent::approval_review::start_with_evidence(
                        self.agent_chat.preferences(),
                        requests,
                        pending.operation.name.clone(),
                        pending.args.clone(),
                        self.session
                            .path()
                            .and_then(Path::parent)
                            .map(Path::to_path_buf),
                        completed,
                    );
                    match result {
                        Ok(job) => self.agent_chat.controls.review = Some((id, job)),
                        Err(error) => {
                            self.agent_chat.push_entry(ChatEntry::Status(
                                self.t(Key::AgentReviewFailed).replace("{reason}", &error),
                            ));
                        }
                    }
                }
                self.agent_chat.restore_pending_focus =
                    !self.panels.is_open(crate::dock::Panel::Agent);
                self.focus_agent_field(AgentField::Chat);
            }
        }
    }

    pub(super) fn agent_approval(&mut self, answer: Approval) {
        let save = self.settings_saver();
        let result = self.agent_approval_with_save(answer, |settings| {
            save(settings).map_err(|error| error.to_string())
        });
        if let Err(error) = result {
            self.agent_chat.push_entry(ChatEntry::Error(error));
        }
        self.focus_agent_field(AgentField::Chat);
    }

    pub(super) fn poll_agent_review(&mut self) -> bool {
        let Some((id, job)) = &self.agent_chat.controls.review else {
            return false;
        };
        let Some(result) = job.poll() else {
            return false;
        };
        let id = *id;
        self.agent_chat.controls.review = None;
        self.apply_agent_review(id, result);
        true
    }

    fn apply_agent_review(
        &mut self,
        id: u64,
        result: Result<auris_agent::approval_review::Review, String>,
    ) {
        let Some(pending) = self
            .agent_chat
            .controls
            .pending
            .as_ref()
            .filter(|pending| pending.id == id)
        else {
            return;
        };
        if self.settings.agent.policy.decide(&pending.operation) != Decision::Review {
            return;
        }
        use auris_agent::approval_review::Verdict;
        match result {
            Ok(review) => {
                let label = self.t(match review.verdict {
                    Verdict::Approve => Key::AgentRuleAllow,
                    Verdict::Ask => Key::AgentAwaitingApproval,
                    Verdict::Deny => Key::AgentRuleDeny,
                });
                self.agent_chat.push_entry(ChatEntry::Status(
                    self.t(Key::AgentReviewResult)
                        .replace("{decision}", label)
                        .replace("{reason}", &review.reason),
                ));
                match review.verdict {
                    Verdict::Approve => self.agent_approval(Approval::Reviewed),
                    Verdict::Deny => self.agent_approval(Approval::Deny),
                    Verdict::Ask => {}
                }
            }
            Err(reason) => {
                self.agent_chat.push_entry(ChatEntry::Status(
                    self.t(Key::AgentReviewFailed).replace("{reason}", &reason),
                ));
            }
        }
    }

    fn agent_approval_with_save(
        &mut self,
        answer: Approval,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), String>,
    ) -> Result<(), String> {
        self.agent_chat.controls.review = None;
        self.agent_chat.restore_pending_focus = false;
        let Some(pending) = self.agent_chat.controls.pending.as_ref() else {
            return Ok(());
        };
        let denied = matches!(answer, Approval::Deny);
        let changed = self.agent_chat.bound_project.as_deref() != self.session.path()
            || (pending.operation.mutating && pending.revision != self.session.revision());
        let forbidden = matches!(
            self.settings.agent.policy.decide(&pending.operation),
            Decision::Deny(_)
        );
        if denied || changed || forbidden {
            let pending = self.agent_chat.controls.pending.take().unwrap();
            self.permission_result(pending.id, false, if changed { "The document changed while confirmation was pending. Inspect it and request approval again." } else { "The operation was denied. Do not retry it unchanged." });
        } else {
            if matches!(answer, Approval::Always) {
                let operation = pending.operation.name.clone();
                let previous = self.settings.agent.clone();
                if let Err(error) = self.settings.agent.policy.set_rule(&operation, Some(true)) {
                    self.settings.agent = previous.clone();
                    self.agent_chat.policy = previous.policy;
                    self.agent_chat.auto_compact_percent = previous.auto_compact_percent;
                    return Err(error);
                }
                self.save_agent_policy_with(previous, save)?;
            }
            let pending = self.agent_chat.controls.pending.take().unwrap();
            if let Some(command) = pending.args.get("command") {
                self.agent_chat
                    .controls
                    .permits
                    .push((command.clone(), pending.revision));
            }
            self.permission_result(
                pending.id,
                true,
                if matches!(answer, Approval::Reviewed) {
                    "Approved by independent automatic review"
                } else {
                    "Explicitly approved by the user"
                },
            );
        }
        Ok(())
    }

    pub(super) fn check_agent_edit(&mut self, command: &serde_json::Value) -> Result<(), String> {
        let operation = Operation::parse("edit_project", &serde_json::json!({"command":command}))?;
        let permit = self
            .agent_chat
            .controls
            .permits
            .iter()
            .position(|(allowed, revision)| {
                allowed == command && *revision == self.session.revision()
            });
        let approved = permit.is_some();
        if let Some(index) = permit {
            self.agent_chat.controls.permits.remove(index);
        }
        match self.settings.agent.policy.decide(&operation) {
            Decision::Allow => Ok(()),
            Decision::Deny(reason) => Err(reason),
            Decision::Ask | Decision::Review if approved => Ok(()),
            Decision::Ask | Decision::Review => {
                Err("This edit needs a fresh confirmation before it can run.".into())
            }
        }
    }

    pub(super) fn agent_control_command(&mut self, text: &str) -> bool {
        let (command, value) = text.split_once(' ').unwrap_or((text, ""));
        let value = value.trim();
        let mode = match command {
            "/read-only" => Some(Mode::ReadOnly),
            "/edit" => Some(Mode::Edit),
            "/plan" => Some(Mode::Plan),
            "/auto-review" => Some(Mode::AutoReview),
            "/bypass" => Some(Mode::Bypass),
            "/mode" => match value {
                "read_only" | "read-only" => Some(Mode::ReadOnly),
                "edit" => Some(Mode::Edit),
                "plan" => Some(Mode::Plan),
                "auto_review" | "auto-review" => Some(Mode::AutoReview),
                "bypass" => Some(Mode::Bypass),
                _ => None,
            },
            _ => None,
        };
        if let Some(mode) = mode {
            self.agent_mode(mode);
        } else {
            match command {
                "/permissions" => {
                    self.set_agent_rules_open(!self.agent_chat.controls.rules_open);
                }
                "/allow" | "/deny" | "/default" => {
                    let allow = match command {
                        "/allow" => Some(true),
                        "/deny" => Some(false),
                        _ => None,
                    };
                    let previous = self.settings.agent.clone();
                    match self.settings.agent.policy.set_rule(value, allow) {
                        Ok(()) => {
                            if self.agent_chat.controls.pending.is_some() {
                                self.agent_approval(Approval::Deny);
                            }
                            self.agent_chat.controls.permits.clear();
                            self.save_agent_policy(previous);
                        }
                        Err(error) => {
                            self.agent_chat.push_entry(ChatEntry::Error(error));
                        }
                    }
                    self.set_agent_rules_open(true);
                }
                "/compact" => self.agent_compact(),
                "/effort" => match ReasoningEffort::named(value) {
                    Some(effort) => self.agent_effort(effort),
                    None => {
                        self.agent_chat.push_entry(ChatEntry::Error(
                            "/effort default | none | minimal | low | medium | high | xhigh | max"
                                .into(),
                        ));
                    }
                },
                "/mode" => {
                    self.agent_chat.push_entry(ChatEntry::Error(
                        "/mode read_only | edit | plan | auto_review | bypass".into(),
                    ));
                }
                _ => return false,
            }
        }
        self.agent_chat.input = TextField::new(String::new());
        true
    }

    pub(super) fn agent_compact(&mut self) {
        if self.agent_operation_busy() {
            return;
        }
        let Some(link) = self.agent_chat.link.as_mut() else {
            self.agent_chat
                .push_entry(ChatEntry::Note(Key::AgentCompactEmpty));
            return;
        };
        if let Err(error) = link.send(r#"{"compact":true}"#) {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
            return;
        }
        self.agent_chat.busy = true;
        self.agent_chat.controls.compacting = true;
    }

    pub(super) fn agent_controls(&mut self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let mode = self.settings.agent.policy.mode;
        let mut mode_names = MODES
            .map(|choice| self.t(mode_key(choice)).to_owned())
            .to_vec();
        mode_names.push(self.t(Key::AgentPermissions).to_owned());
        div()
            .w_full()
            .min_w_0()
            .child(self.dropdown(
                "agent-mode-menu",
                self.t(mode_key(mode)).to_owned(),
                self.agent_chat.controls.mode_menu,
                &theme,
                |this, _| {
                    if !this.agent_operation_busy() {
                        this.agent_chat.controls.mode_menu = !this.agent_chat.controls.mode_menu;
                        this.agent_chat.controls.effort_menu = false;
                        this.agent_chat.model_menu = false;
                    }
                },
                cx,
            ))
            .when(self.agent_chat.controls.mode_menu, |this| {
                this.child(self.option_rows(
                    "agent-mode-option",
                    &mode_names,
                    MODES.iter().position(|choice| *choice == mode).unwrap_or(0),
                    &theme,
                    |this, index, _| {
                        if let Some(mode) = MODES.get(index).copied() {
                            this.agent_mode(mode);
                        } else {
                            this.agent_chat.controls.mode_menu = false;
                            this.set_agent_rules_open(!this.agent_chat.controls.rules_open);
                        }
                    },
                    cx,
                ))
            })
            .into_any_element()
    }
    pub(super) fn agent_rules(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let operation_busy = self.agent_operation_busy();
        let mut content = div()
            .flex()
            .flex_col()
            .gap_1()
            .p_2()
            .text_xs()
            .child(self.t(Key::AgentPermissionHelp));
        for operation in ["*", "edit_project.*"]
            .into_iter()
            .chain(OPERATIONS.iter().copied())
        {
            let broad = matches!(operation, "*" | "edit_project.*");
            let allow = self
                .settings
                .agent
                .policy
                .allow
                .iter()
                .any(|name| name == operation);
            let deny = self
                .settings
                .agent
                .policy
                .deny
                .iter()
                .any(|name| name == operation);
            let state = if deny {
                Key::AgentRuleDeny
            } else if allow {
                Key::AgentRuleAllow
            } else {
                Key::AgentRuleDefault
            };
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .flex_wrap()
                    .child(div().flex_1().min_w_0().child(operation))
                    .child(button_enabled(
                        SharedString::from(format!("agent-rule-{operation}")),
                        self.t(state),
                        ButtonStyle::Normal,
                        ButtonState::available(allow || deny, !operation_busy),
                        if deny { theme.danger } else { theme.accent },
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            let next = if deny {
                                None
                            } else if allow || broad {
                                Some(false)
                            } else {
                                Some(true)
                            };
                            let previous = this.settings.agent.clone();
                            if this.settings.agent.policy.set_rule(operation, next).is_ok() {
                                if this.agent_chat.controls.pending.is_some() {
                                    this.agent_approval(Approval::Deny);
                                }
                                this.agent_chat.controls.permits.clear();
                                this.save_agent_policy(previous);
                            }
                            cx.notify();
                        }),
                    )),
            );
        }
        let percent = self.settings.agent.auto_compact_percent.unwrap_or(85);
        content
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .items_center()
                    .child(self.t(Key::AgentAutoCompact))
                    .child(button_enabled(
                        "agent-auto-compact",
                        if percent == 0 {
                            self.t(Key::AgentCompactOff).to_string()
                        } else {
                            format!("{percent}%")
                        },
                        ButtonStyle::Normal,
                        ButtonState::available(percent > 0, !operation_busy),
                        theme.accent,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            let previous = this.settings.agent.clone();
                            let value = match this.settings.agent.auto_compact_percent.unwrap_or(85)
                            {
                                0 => 70,
                                70 => 85,
                                _ => 0,
                            };
                            this.settings.agent.auto_compact_percent = Some(value);
                            this.agent_chat.auto_compact_percent = Some(value);
                            this.save_agent_policy(previous);
                            cx.notify();
                        }),
                    )),
            )
            .into_any_element()
    }

    pub(super) fn agent_approval_view(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let Some(pending) = &self.agent_chat.controls.pending else {
            return div().into_any_element();
        };
        let theme = &self.theme;
        // One answer leads. "Always" outlives this request, so it is the quietest of the three,
        // set apart at the far end rather than sitting beside "once" as its equal.
        let respond = |id: &'static str, key: Key, style: ButtonStyle, answer: Approval| {
            button(
                id,
                self.t(key),
                style,
                false,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.agent_approval(answer);
                    cx.notify();
                }),
            )
        };
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .child(respond(
                "agent-allow-once",
                Key::AgentAllowOnce,
                ButtonStyle::Primary,
                Approval::Once,
            ))
            .child(respond(
                "agent-deny",
                Key::AgentDenyOnce,
                ButtonStyle::Normal,
                Approval::Deny,
            ))
            .child(div().flex_1())
            .child(respond(
                "agent-allow-always",
                Key::AgentAllowAlways,
                ButtonStyle::Ghost,
                Approval::Always,
            ));
        // Tables and commands in arguments line up only in a fixed-width face; one line per
        // div, because a div's text collapses the newlines.
        let arguments = serde_json::to_string_pretty(&pending.args).unwrap_or_default();
        let mono = {
            use gpui_component::ActiveTheme as _;
            cx.theme().mono_font_family.clone()
        };
        div()
            .id("agent-approval")
            .debug_selector(|| "agent-approval".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .p_2()
            .border_t_2()
            .border_color(theme.warning)
            .bg(theme.surface_raised)
            .text_xs()
            .text_color(theme.text_muted)
            .child(
                div()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(theme.text)
                    .child(self.t(Key::AgentApprovalTitle)),
            )
            .when_some(approval_summary(&pending.operation), |this, summary| {
                this.child(
                    div()
                        .id("agent-approval-summary")
                        .debug_selector(|| "agent-approval-summary".into())
                        .text_color(theme.text)
                        .child(self.t(summary)),
                )
            })
            .child(format!(
                "{} — {}",
                self.project().name,
                pending.operation.name
            ))
            .when(self.agent_chat.controls.review.is_some(), |this| {
                this.child(self.t(Key::AgentReviewing))
            })
            .when(pending.operation.name.starts_with("cli_"), |this| {
                this.child(
                    div()
                        .text_color(theme.warning)
                        .child(self.t(Key::AgentExternalOperation)),
                )
            })
            .child(
                div()
                    .text_color(theme.text_faint)
                    .child(self.t(Key::AgentToolArguments)),
            )
            .child(
                div()
                    .id("agent-approval-details")
                    .debug_selector(|| "agent-approval-details".into())
                    .flex()
                    .flex_col()
                    .max_h_32()
                    .overflow_y_scroll()
                    .p_1()
                    .rounded(Metrics::RADIUS_SM)
                    .bg(theme.surface_sunken)
                    .font_family(mono)
                    .text_color(theme.text)
                    .children(
                        arguments
                            .lines()
                            .map(|line| div().child(line.to_string()))
                            .collect::<Vec<_>>(),
                    ),
            )
            .child(
                self.t(Key::AgentApprovalKeys)
                    .replace("{once}", &crate::actions::menu_keystroke("secondary-enter"))
                    .replace(
                        "{always}",
                        &crate::actions::menu_keystroke("secondary-shift-enter"),
                    ),
            )
            .child(actions)
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn wire_permissions_retain_call_identity_across_allow_deny_cancel_and_multiple_checks(
        cx: &mut gpui::TestAppContext,
    ) {
        use auris_session::execution_trace::TraceOperation;
        use serde_json::{Value, json};
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            // This fixture owns its permission policy, independently of concurrent settings tests.
            this.settings.agent.policy = Default::default();
            this.session.execution_trace(TraceOperation::Start).unwrap();
            let dispatch = |this: &mut AurisApp, value: Value| {
                let event = parse_event(&value.to_string()).unwrap();
                this.route_agent_control_event(&event)
            };
            let command = json!({"action":"import_transcription","name":"Private fixture label","at_tick":0,
                "draft":{"kind":"piano","transcription":{"source":{"method":"cpu-fixture"},"seconds":1.0,
                "notes":[{"pitch":60,"start":0.0,"end":1.0,"strength":0.7}]}}});
            let request = |id, call: &str| json!({"event":"permission","id":id,"tool":"edit_project","args":{"command":command},"origin":{"kind":"tool_call","call_id":call,"tool":"import_transcription"}});
            this.settings.agent.policy.mode = Mode::Edit;
            dispatch(this, json!({"event":"call","call_id":"allowed","tool":"import_transcription","args":{}}));
            assert!(dispatch(this, request(1, "allowed")));
            assert!(dispatch(this, request(2, "allowed")));
            this.agent_edit(command.clone()).unwrap();
            dispatch(this, json!({"event":"result","call_id":"allowed","tool":"import_transcription","ok":true,"text":"private output"}));
            this.settings.agent.policy.mode = Mode::ReadOnly;
            dispatch(this, json!({"event":"call","call_id":"denied","tool":"import_transcription","args":{}}));
            dispatch(this, request(3, "denied"));
            dispatch(this, json!({"event":"result","call_id":"denied","tool":"import_transcription","ok":false,"text":"denied"}));
            this.settings.agent.policy.mode = Mode::Edit;
            dispatch(this, json!({"event":"call","call_id":"cancelled","tool":"cli_command","args":{}}));
            dispatch(this, json!({"event":"permission","id":4,"tool":"cli_command","args":{"backend":"fixture","input":{"command":"private command"}},"origin":{"kind":"tool_call","call_id":"cancelled","tool":"cli_command"}}));
            assert!(this.agent_chat.controls.pending.is_some());
            this.agent_stop(cx);
            dispatch(this, json!({"event":"result","call_id":"cancelled","tool":"cli_command","ok":false,"text":"late cancellation"}));
            dispatch(this, json!({"event":"permission","id":5,"tool":"inspect_project","args":{},"origin":{"kind":"independent","source":"startup"}}));
            let trace: Value = serde_json::from_str(&this.session.execution_trace(TraceOperation::Read { after: None }).unwrap()).unwrap();
            let events = trace["events"].as_array().unwrap();
            for (id, call, outcome) in [(1, "allowed", "allowed"), (2, "allowed", "allowed"), (3, "denied", "denied"), (4, "cancelled", "cancelled")] {
                let permission = events.iter().find(|e| e["kind"] == "permission_outcome" && e["permission_id"] == id).unwrap();
                assert_eq!(permission["call_id"], call);
                assert_eq!(permission["outcome"], outcome);
                assert_eq!(permission["tool"], if id == 4 { "cli_command" } else { "import_transcription" });
            }
            let result = events.iter().find(|e| e["kind"] == "host_outcome" && e["call_id"] == "allowed").unwrap();
            assert_eq!(result["outcome"], "success");
            assert_eq!(events.iter().filter(|e| e["kind"] == "host_outcome" && e["call_id"] == "cancelled").count(), 1);
            assert!(result["before_state"].is_string());
            assert_ne!(result["before_state"], result["state"]);
            assert!(events.iter().any(|e| e["permission_id"] == 5 && e["call_id"].is_null() && e["origin"]["source"] == "startup"));
            assert!(!trace.to_string().contains("private command"));
            assert!(!trace.to_string().contains("private output"));
            assert!(!trace.to_string().contains("Private fixture label"));
        });
    }

    #[test]
    fn review_history_excludes_pending_failed_and_non_native_tool_calls() {
        let entry = |name: &str, ok, detail: &str| ChatEntry::Tool {
            name: name.into(),
            args: "{\"command\":\"hash analysis.py\"}".into(),
            ok,
            line: String::new(),
            detail: detail.into(),
        };
        let entries = vec![
            entry("cli_command", true, "hash result"),
            entry("cli_command", true, ""),
            entry("cli_command", false, "failure"),
            entry("inspect_project", true, "project"),
            entry("cli_file_change", true, "changed script"),
        ];
        let history = completed_review_operations(&entries);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["result"], "hash result");
        assert_eq!(history[1]["result"], "changed script");
        assert_eq!(history[0]["arguments"]["command"], "hash analysis.py");
    }

    #[test]
    fn automatic_review_weighs_the_newest_typed_requests_within_its_budget() {
        let typed = vec![
            "first".to_string(),
            "x".repeat(20 * 1024),
            "y".repeat(20 * 1024),
            "last".to_string(),
        ];
        let requests = review_requests(&typed);
        assert_eq!(requests, ["y".repeat(20 * 1024), "last".to_string()]);
        assert!(review_requests(&[]).is_empty());
    }

    #[gpui::test]
    fn auto_review_is_an_explicit_mode_and_permissions_keep_their_own_menu_row(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".into());
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-mode-menu", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-mode-option-3", cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.settings.agent.policy.mode, Mode::AutoReview)
        });
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-mode-menu", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-permissions", cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.controls.rules_open);
            assert_eq!(this.settings.agent.policy.mode, Mode::AutoReview);
        });
    }

    #[gpui::test]
    fn reviewed_approvals_are_one_time_and_uncertainty_keeps_manual_confirmation(
        cx: &mut gpui::TestAppContext,
    ) {
        use auris_agent::approval_review::{Review, Verdict};
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = PolicyForTest::default();
            let track = this
                .session
                .add_default_instrument_track("Reviewed lead")
                .unwrap();
            let command = command(track.0);
            let args = serde_json::json!({"command":command.clone()});
            this.agent_permission(101, "edit_project".into(), args);
            this.settings.agent.policy.mode = Mode::AutoReview;
            for result in [
                Ok(Review {
                    verdict: Verdict::Ask,
                    reason: "Needs consent".into(),
                }),
                Err("offline".into()),
            ] {
                this.apply_agent_review(101, result);
                assert!(this.agent_chat.controls.pending.is_some());
                assert!(this.agent_edit(command.clone()).is_err());
            }
            this.apply_agent_review(
                99,
                Ok(Review {
                    verdict: Verdict::Approve,
                    reason: "Stale request".into(),
                }),
            );
            assert!(this.agent_chat.controls.pending.is_some());
            this.apply_agent_review(
                101,
                Ok(Review {
                    verdict: Verdict::Approve,
                    reason: "Requested scoped edit".into(),
                }),
            );
            assert!(this.agent_chat.controls.pending.is_none());
            assert!(this.settings.agent.policy.allow.is_empty());
            this.agent_edit(command.clone()).unwrap();
            assert!(this.agent_edit(command).is_err());
        });
    }

    #[gpui::test]
    fn reviewed_approvals_cannot_survive_document_changes_denial_or_cancellation(
        cx: &mut gpui::TestAppContext,
    ) {
        use auris_agent::approval_review::{Review, Verdict};
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            let track = this
                .session
                .add_default_instrument_track("Keep lead")
                .unwrap();
            let command = command(track.0);
            for scenario in ["revision", "deny", "cancel", "review_denied"] {
                this.settings.agent.policy = PolicyForTest::default();
                this.agent_permission(
                    102,
                    "edit_project".into(),
                    serde_json::json!({"command":command.clone()}),
                );
                this.settings.agent.policy.mode = Mode::AutoReview;
                match scenario {
                    "revision" => {
                        this.session
                            .add_default_instrument_track("Concurrent edit")
                            .unwrap();
                    }
                    "deny" => {
                        this.settings
                            .agent
                            .policy
                            .set_rule("edit_project.remove_track", Some(false))
                            .unwrap();
                    }
                    "cancel" => this.agent_approval(Approval::Deny),
                    _ => {}
                }
                this.apply_agent_review(
                    102,
                    Ok(Review {
                        verdict: if scenario == "review_denied" {
                            Verdict::Deny
                        } else {
                            Verdict::Approve
                        },
                        reason: scenario.into(),
                    }),
                );
                assert!(this.agent_edit(command.clone()).is_err(), "{scenario}");
                assert!(this.project().track(track).is_some());
                this.agent_chat.controls.pending = None;
            }
        });
    }

    fn command(track: u64) -> serde_json::Value {
        serde_json::json!({"action":"remove_track","track":track})
    }

    #[gpui::test]
    fn broad_permission_rules_cannot_be_allowed_with_one_click(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.models_loaded = true;
            this.agent_chat.controls.rules_open = true;
            this.settings.agent.policy = PolicyForTest::default();
        });
        for (operation, selector) in [
            ("*", "agent-rule-*"),
            ("edit_project.*", "agent-rule-edit_project.*"),
        ] {
            crate::harness::paint(&app, cx);
            crate::harness::click(selector, cx);
            app.read_with(cx, |this, _| {
                assert!(
                    !this
                        .settings
                        .agent
                        .policy
                        .allow
                        .iter()
                        .any(|rule| rule == operation),
                    "{operation} became a persistent allow rule after one click"
                );
            });
            app.update(cx, |this, _| {
                let previous = this.settings.agent.clone();
                this.settings.agent.policy = PolicyForTest::default();
                this.save_agent_policy(previous);
            });
        }
    }

    #[gpui::test]
    fn permission_menu_reveals_rules_and_returns_to_the_transcript(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.model = "offline-fixture".into();
            this.agent_chat.models_loaded = true;
            this.agent_chat.models_error = Some("offline fixture".into());
            this.agent_chat.entries = (0..40)
                .map(|index| ChatEntry::Status(format!("older line {index}")))
                .collect();
        });
        crate::harness::paint(&app, cx);
        app.update(cx, |this, _| {
            this.agent_chat
                .scroll
                .set_offset(gpui::point(px(0.0), px(-120.0)));
        });

        crate::harness::click("agent-mode-menu", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-permissions", cx);
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.controls.rules_open);
            assert_eq!(this.agent_chat.scroll.offset().y, px(0.0));
        });
        assert!(cx.debug_bounds("agent-rule-*").is_some());

        crate::harness::click("agent-mode-menu", cx);
        crate::harness::paint(&app, cx);
        crate::harness::click("agent-permissions", cx);
        app.read_with(cx, |this, _| assert!(!this.agent_chat.controls.rules_open));
    }
    #[gpui::test]
    fn shift_tab_moves_focus_without_answering_a_pending_approval(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_chat.models_loaded = true;
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::Edit;
            this.agent_chat.focused = Some(AgentField::Chat);
            this.agent_permission(
                7,
                "search_internet".into(),
                serde_json::json!({"query":"approval shortcut"}),
            );
            assert!(this.agent_chat.controls.pending.is_some());
        });

        crate::harness::paint(&app, cx);
        crate::harness::click("agent-input", cx);
        cx.simulate_keystrokes("shift-tab");

        cx.update(|window, cx| {
            app.read_with(cx, |this, cx| {
                assert!(this.agent_chat.controls.pending.is_some());
                assert_eq!(this.settings.agent.policy.mode, Mode::Edit);
                assert!(this.pane_focused(crate::app::Pane::Agent, window, cx));
            });
        });
    }

    #[gpui::test]
    fn reselecting_the_current_mode_keeps_a_pending_approval(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::Edit;
            this.agent_permission(
                9,
                "search_internet".into(),
                serde_json::json!({"query":"same mode"}),
            );
            assert!(this.agent_chat.controls.pending.is_some());

            this.agent_mode(Mode::Edit);

            assert!(this.agent_chat.controls.pending.is_some());
            assert_eq!(this.settings.agent.policy.mode, Mode::Edit);
        });
    }

    #[gpui::test]
    fn a_hidden_approval_restores_keys_once_without_reclaiming_later_focus(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.hide(crate::dock::Panel::Agent);
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::Edit;
            this.agent_permission(
                8,
                "search_internet".into(),
                serde_json::json!({"query":"hidden approval"}),
            );
        });
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.controls.pending.is_some());
            assert!(this.agent_chat.focused.is_none());
            assert!(this.agent_chat.restore_pending_focus);
        });

        app.update(cx, |this, _| this.panels.show(crate::dock::Panel::Agent));
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, _| {
                assert!(
                    this.agent_chat
                        .input_focus()
                        .is_some_and(|focus| focus.is_focused(window))
                );
                assert_eq!(this.agent_chat.focused, Some(AgentField::Chat));
                assert!(!this.agent_chat.restore_pending_focus);
            });
        });

        cx.update(|window, cx| {
            app.update(cx, |this, _| {
                this.focus_pane(crate::app::Pane::Arrangement, window)
            });
        });
        crate::harness::paint(&app, cx);
        cx.update(|window, cx| {
            app.read_with(cx, |this, cx| {
                assert!(this.agent_chat.controls.pending.is_some());
                assert!(this.agent_chat.focused.is_none());
                assert!(this.pane_focused(crate::app::Pane::Arrangement, window, cx));
            });
        });

        // Hiding a visible, already-pending approval is the same suspended state as receiving
        // it while hidden. Returning to the panel restores the advertised keys one more time.
        app.update(cx, |this, _| this.panels.hide(crate::dock::Panel::Agent));
        crate::harness::paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.agent_chat.restore_pending_focus));
        app.update(cx, |this, _| this.panels.show(crate::dock::Panel::Agent));
        crate::harness::paint(&app, cx);
        cx.simulate_keystrokes("escape");
        app.read_with(cx, |this, _| {
            assert!(this.agent_chat.controls.pending.is_none());
            assert!(!this.agent_chat.restore_pending_focus);
        });
    }

    #[gpui::test]
    fn saved_retired_permission_rules_allow_live_inspection_and_composition_edits(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = serde_json::from_value(serde_json::json!({
                "mode":"bypass", "allow":["edit_project.listen", "edit_project.remove_notes"]
            }))
            .unwrap();
            this.agent_edit(serde_json::json!({"action":"inspect"}))
                .unwrap();
            let tracks = this.project().tracks.len();
            this.agent_edit(serde_json::json!({
                "action":"add_track", "name":"Summer vocal", "kind":"instrument"
            }))
            .unwrap();
            assert_eq!(this.project().tracks.len(), tracks + 1);
            assert!(this.session.path().is_none());
            this.session.undo();
            assert_eq!(this.project().tracks.len(), tracks);
        });
    }

    #[gpui::test]
    fn approval_is_visible_and_allows_exactly_one_edit(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let (command, track) = app.update(cx, |this, _| {
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::Edit;
            this.panels.show(crate::dock::Panel::Agent);
            let track = this
                .session
                .add_default_instrument_track("Temporary lead")
                .unwrap();
            let command = command(track.0);
            this.agent_permission(
                1,
                "edit_project".into(),
                serde_json::json!({"command":command.clone()}),
            );
            assert!(this.agent_chat.controls.pending.is_some());
            assert!(this.agent_edit(command.clone()).is_err());
            (command, track)
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-approval").is_some());
        assert!(
            cx.debug_bounds("agent-approval-summary").is_some(),
            "removing a track is described, not only named"
        );
        let once = cx.debug_bounds("agent-allow-once").unwrap();
        let deny = cx.debug_bounds("agent-deny").unwrap();
        assert!(once.left() < deny.left(), "allowing once leads");
        assert!(cx.debug_bounds("agent-allow-always").is_some());
        crate::harness::click("agent-allow-once", cx);
        app.update(cx, |this, _| {
            this.agent_edit(command.clone()).unwrap();
            assert!(this.agent_edit(command.clone()).is_err());
            assert!(this.project().track(track).is_none());
            assert!(this.session.path().is_none());
        });
    }

    #[gpui::test]
    fn stale_or_denied_approvals_cannot_change_the_document(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::Edit;
            let track = this
                .session
                .add_default_instrument_track("Protected lead")
                .unwrap();
            let command = command(track.0);
            this.agent_permission(
                2,
                "edit_project".into(),
                serde_json::json!({"command":command.clone()}),
            );
            this.session
                .agent_command(
                    serde_json::from_value(serde_json::json!({
                        "action":"add_track",
                        "name":"Concurrent edit",
                        "kind":"instrument"
                    }))
                    .unwrap(),
                )
                .unwrap();
            let before = this.project().clone();
            this.agent_approval(Approval::Once);
            assert!(this.agent_edit(command.clone()).is_err());
            assert_eq!(this.project(), &before);
            this.agent_permission(
                3,
                "edit_project".into(),
                serde_json::json!({"command":command.clone()}),
            );
            this.agent_approval(Approval::Once);
            this.settings.agent.policy.mode = Mode::Bypass;
            this.settings
                .agent
                .policy
                .set_rule("edit_project.*", Some(false))
                .unwrap();
            assert!(this.agent_edit(command).is_err());
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn always_approval_rolls_back_and_remains_actionable_when_policy_save_fails(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::Edit;
            this.agent_chat.policy = this.settings.agent.policy.clone();
            let before = this.settings.agent.policy.clone();
            let command = command(42);
            this.agent_permission(
                99,
                "edit_project".into(),
                serde_json::json!({"command":command}),
            );
            assert!(this.agent_chat.controls.pending.is_some());

            let result = this.agent_approval_with_save(Approval::Always, |_| {
                Err("settings fixture failure".to_string())
            });

            assert_eq!(result, Err("settings fixture failure".to_string()));
            assert_eq!(this.settings.agent.policy, before);
            assert_eq!(this.agent_chat.policy, before);
            assert!(this.agent_chat.controls.pending.is_some());
            assert!(this.agent_chat.controls.permits.is_empty());
        });
    }

    use auris_session::agent_policy::Policy as PolicyForTest;

    #[test]
    fn slash_completion_covers_commands_arguments_and_plain_text() {
        let commands = slash_matches("/m");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].fill, "/mode ");

        let modes = slash_matches("/mode p");
        assert_eq!(modes.len(), 1);
        assert_eq!(modes[0].fill, "/mode plan");

        let effort = slash_matches("/effort h");
        assert_eq!(effort.len(), 1);
        assert_eq!(effort[0].fill, "/effort high");
        assert!(slash_matches("write a chorus").is_empty());
    }

    #[test]
    fn only_bypass_warns_and_summaries_name_only_what_is_certain() {
        assert!(mode_warns(Mode::Bypass));
        for mode in [Mode::ReadOnly, Mode::Edit, Mode::Plan, Mode::AutoReview] {
            assert!(!mode_warns(mode), "{mode:?}");
        }
        let operation = |name: &str, mutating, confirm| Operation {
            name: name.into(),
            mutating,
            confirm,
        };
        for (name, mutating, confirm, expected) in [
            (
                "cli_command",
                true,
                true,
                Some(Key::AgentApprovalRunCommand),
            ),
            (
                "cli_file_change",
                true,
                true,
                Some(Key::AgentApprovalChangeFiles),
            ),
            (
                "cli_file_read",
                false,
                true,
                Some(Key::AgentApprovalReadOutside),
            ),
            (
                "search_internet",
                false,
                true,
                Some(Key::AgentApprovalInternet),
            ),
            (
                "edit_project.remove_track",
                true,
                true,
                Some(Key::AgentApprovalReplaces),
            ),
            (
                "edit_project.add_track",
                true,
                false,
                Some(Key::AgentApprovalEdits),
            ),
            ("edit_project.render", false, true, None),
        ] {
            assert_eq!(
                approval_summary(&operation(name, mutating, confirm)),
                expected,
                "{name}"
            );
        }
    }

    #[gpui::test]
    fn bypass_mode_says_so_for_as_long_as_it_is_chosen(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.panels.show(crate::dock::Panel::Agent);
            this.settings.agent.policy.mode = Mode::Edit;
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-bypass-warning").is_none());

        app.update(cx, |this, _| this.settings.agent.policy.mode = Mode::Bypass);
        crate::harness::paint(&app, cx);
        let warning = cx
            .debug_bounds("agent-bypass-warning")
            .expect("bypass is announced");
        let composer = cx.debug_bounds("agent-composer").unwrap();
        assert!(warning.top() >= composer.top() && warning.bottom() <= composer.bottom());
    }
}
