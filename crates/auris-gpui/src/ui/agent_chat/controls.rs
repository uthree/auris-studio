//! Permission decisions and controls owned by the desktop session.
use super::*;
use auris_session::agent_policy::{Decision, Mode, OPERATIONS, Operation};

#[derive(Default)]
pub(super) struct Controls {
    pub(super) pending: Option<Pending>,
    pub(super) permits: Vec<(serde_json::Value, u64)>,
    pub(super) rules_open: bool,
    pub(super) compacting: bool,
    pub(super) mode_menu: bool,
    pub(super) effort_menu: bool,
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

#[derive(Clone, Copy)]
pub(super) enum Approval {
    Once,
    Always,
    Deny,
}

fn mode_key(mode: Mode) -> Key {
    match mode {
        Mode::ReadOnly => Key::AgentReadOnly,
        Mode::Edit => Key::AgentEditMode,
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

pub(super) fn slash_matches(value: &str) -> Vec<SlashMatch> {
    if !value.starts_with('/') || value.contains('\n') {
        return Vec::new();
    }
    let commands = [
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
        ("/bypass", false, Key::AgentSlashMode),
    ];
    let Some((command, argument)) = value.split_once(' ') else {
        return commands
            .into_iter()
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
            vec!["read-only", "edit", "plan", "bypass"],
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

impl AurisApp {
    fn save_agent_policy(&mut self) {
        self.agent_chat.policy = self.settings.agent.policy.clone();
        if let Err(error) = self.settings.save() {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
        }
    }

    pub(super) fn agent_mode(&mut self, mode: Mode) {
        // Changing permissions invalidates previously approved operations.
        if self.agent_chat.controls.pending.is_some() {
            self.agent_approval(Approval::Deny);
        }
        self.agent_chat.controls.permits.clear();
        self.settings.agent.policy.mode = mode;
        self.agent_chat.controls.mode_menu = false;
        self.save_agent_policy();
        self.focus_agent_field(AgentField::Chat);
    }

    pub(super) fn agent_effort(&mut self, effort: ReasoningEffort) {
        self.agent_chat.effort = effort;
        self.agent_chat.thinking = None;
        self.agent_chat.controls.effort_menu = false;
        self.agent_write_through();
        self.focus_agent_field(AgentField::Chat);
    }

    fn permission_result(&mut self, id: u64, ok: bool, reason: &str) {
        let wire =
            serde_json::json!({"event":"permission_result", "id":id, "ok":ok, "reason":reason});
        if let Some(link) = self.agent_chat.link.as_mut()
            && let Err(error) = link.send(&wire.to_string())
        {
            self.agent_chat
                .push_entry(ChatEntry::Error(error.to_string()));
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
            Decision::Ask => {
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
                self.focus_agent_field(AgentField::Chat);
            }
        }
    }

    pub(super) fn agent_approval(&mut self, answer: Approval) {
        let Some(pending) = self.agent_chat.controls.pending.take() else {
            return;
        };
        let denied = matches!(answer, Approval::Deny);
        let changed = self.agent_chat.bound_project.as_deref() != self.session.path()
            || (pending.operation.mutating && pending.revision != self.session.revision());
        let forbidden = matches!(
            self.settings.agent.policy.decide(&pending.operation),
            Decision::Deny(_)
        );
        if denied || changed || forbidden {
            self.permission_result(pending.id, false, if changed { "The document changed while confirmation was pending. Inspect it and request approval again." } else { "The operation was denied. Do not retry it unchanged." });
        } else {
            if matches!(answer, Approval::Always) {
                if let Err(error) = self
                    .settings
                    .agent
                    .policy
                    .set_rule(&pending.operation.name, Some(true))
                {
                    self.permission_result(pending.id, false, &error);
                    return;
                }
                self.save_agent_policy();
            }
            if let Some(command) = pending.args.get("command") {
                self.agent_chat
                    .controls
                    .permits
                    .push((command.clone(), pending.revision));
            }
            self.permission_result(pending.id, true, "Explicitly approved by the user");
        }
        self.focus_agent_field(AgentField::Chat);
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
            Decision::Ask if approved => Ok(()),
            Decision::Ask => Err("This edit needs a fresh confirmation before it can run.".into()),
        }
    }

    pub(super) fn agent_control_command(&mut self, text: &str) -> bool {
        let (command, value) = text.split_once(' ').unwrap_or((text, ""));
        let value = value.trim();
        let mode = match command {
            "/read-only" => Some(Mode::ReadOnly),
            "/edit" => Some(Mode::Edit),
            "/plan" => Some(Mode::Plan),
            "/bypass" => Some(Mode::Bypass),
            "/mode" => match value {
                "read_only" | "read-only" => Some(Mode::ReadOnly),
                "edit" => Some(Mode::Edit),
                "plan" => Some(Mode::Plan),
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
                    self.agent_chat.controls.rules_open = !self.agent_chat.controls.rules_open
                }
                "/allow" | "/deny" | "/default" => {
                    let allow = match command {
                        "/allow" => Some(true),
                        "/deny" => Some(false),
                        _ => None,
                    };
                    match self.settings.agent.policy.set_rule(value, allow) {
                        Ok(()) => {
                            if self.agent_chat.controls.pending.is_some() {
                                self.agent_approval(Approval::Deny);
                            }
                            self.agent_chat.controls.permits.clear();
                            self.save_agent_policy();
                        }
                        Err(error) => {
                            self.agent_chat.push_entry(ChatEntry::Error(error));
                        }
                    }
                    self.agent_chat.controls.rules_open = true;
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
                        "/mode read_only | edit | plan | bypass".into(),
                    ));
                }
                _ => return false,
            }
        }
        self.agent_chat.input = TextField::new(String::new());
        true
    }

    pub(super) fn agent_compact(&mut self) {
        if self.agent_chat.busy {
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

    pub(super) fn agent_controls(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let mode = self.settings.agent.policy.mode;
        let effort = self.settings.agent.effort;
        let mut row = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(theme.border);
        row = row
            .child(div().w(px(140.0)).child(self.dropdown(
                "agent-mode-menu",
                format!(
                    "{}: {}",
                    self.t(Key::AgentApprovalMode),
                    self.t(mode_key(mode))
                ),
                self.agent_chat.controls.mode_menu,
                theme,
                |this, _| {
                    this.agent_chat.controls.mode_menu = !this.agent_chat.controls.mode_menu;
                    this.agent_chat.controls.effort_menu = false;
                },
                cx,
            )))
            .child(div().w(px(120.0)).child(self.dropdown(
                "agent-effort-menu",
                format!(
                    "{}: {}",
                    self.t(Key::AgentEffort),
                    self.t(effort_key(effort))
                ),
                self.agent_chat.controls.effort_menu,
                theme,
                |this, _| {
                    this.agent_chat.controls.effort_menu = !this.agent_chat.controls.effort_menu;
                    this.agent_chat.controls.mode_menu = false;
                },
                cx,
            )));
        row = row.child(button(
            "agent-permissions",
            self.t(Key::AgentPermissions),
            ButtonStyle::Normal,
            self.agent_chat.controls.rules_open,
            theme.accent,
            theme,
            cx.listener(|this, _, _, cx| {
                this.agent_chat.controls.rules_open = !this.agent_chat.controls.rules_open;
                this.focus_agent_field(AgentField::Chat);
                cx.notify();
            }),
        ));
        if !self.agent_chat.busy {
            row = row.child(button(
                "agent-compact",
                self.t(Key::AgentCompact),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| {
                    this.agent_compact();
                    cx.notify();
                }),
            ));
        }
        let mode_names = [Mode::ReadOnly, Mode::Edit, Mode::Plan, Mode::Bypass]
            .map(|choice| self.t(mode_key(choice)).to_string());
        let efforts = effort_choices(self.agent_chat.provider_openai);
        let effort_names: Vec<String> = efforts
            .iter()
            .map(|choice| self.t(effort_key(*choice)).to_string())
            .collect();
        div()
            .flex()
            .flex_col()
            .child(row)
            .when(self.agent_chat.controls.mode_menu, |this| {
                this.child(self.option_rows(
                    "agent-mode-option",
                    &mode_names,
                    theme,
                    |this, index, _| {
                        if let Some(mode) = [Mode::ReadOnly, Mode::Edit, Mode::Plan, Mode::Bypass]
                            .get(index)
                            .copied()
                        {
                            this.agent_mode(mode);
                        }
                    },
                    cx,
                ))
            })
            .when(self.agent_chat.controls.effort_menu, |this| {
                this.child(self.option_rows(
                    "agent-effort-option",
                    &effort_names,
                    theme,
                    move |this, index, _| {
                        if let Some(effort) = effort_choices(this.agent_chat.provider_openai)
                            .get(index)
                            .copied()
                        {
                            this.agent_effort(effort);
                        }
                    },
                    cx,
                ))
            })
            .into_any_element()
    }

    pub(super) fn agent_rules(&self, cx: &mut gpui::Context<Self>) -> AnyElement {
        let theme = &self.theme;
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
                    .child(button(
                        SharedString::from(format!("agent-rule-{operation}")),
                        self.t(state),
                        ButtonStyle::Normal,
                        allow || deny,
                        if deny { theme.danger } else { theme.accent },
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            let next = if deny {
                                None
                            } else if allow {
                                Some(false)
                            } else {
                                Some(true)
                            };
                            if this.settings.agent.policy.set_rule(operation, next).is_ok() {
                                if this.agent_chat.controls.pending.is_some() {
                                    this.agent_approval(Approval::Deny);
                                }
                                this.agent_chat.controls.permits.clear();
                                this.save_agent_policy();
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
                    .child(button(
                        "agent-auto-compact",
                        if percent == 0 {
                            self.t(Key::AgentCompactOff).to_string()
                        } else {
                            format!("{percent}%")
                        },
                        ButtonStyle::Normal,
                        percent > 0,
                        theme.accent,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            let value = match this.settings.agent.auto_compact_percent.unwrap_or(85)
                            {
                                0 => 70,
                                70 => 85,
                                _ => 0,
                            };
                            this.settings.agent.auto_compact_percent = Some(value);
                            this.agent_chat.auto_compact_percent = Some(value);
                            this.save_agent_policy();
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
        let mut actions = div().flex().flex_wrap().gap_1();
        for (id, key, answer) in [
            ("agent-deny", Key::AgentDenyOnce, Approval::Deny),
            ("agent-allow-once", Key::AgentAllowOnce, Approval::Once),
            (
                "agent-allow-always",
                Key::AgentAllowAlways,
                Approval::Always,
            ),
        ] {
            actions = actions.child(button(
                id,
                self.t(key),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.agent_approval(answer);
                    cx.notify();
                }),
            ));
        }
        div()
            .id("agent-approval")
            .debug_selector(|| "agent-approval".into())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .p_2()
            .border_t_1()
            .border_color(theme.warning)
            .bg(theme.surface_raised)
            .text_xs()
            .child(self.t(Key::AgentApprovalTitle))
            .child(format!(
                "{} — {}",
                self.project().name,
                pending.operation.name
            ))
            .child(
                div()
                    .id("agent-approval-details")
                    .max_h_32()
                    .overflow_y_scroll()
                    .child(serde_json::to_string_pretty(&pending.args).unwrap_or_default()),
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

    fn command() -> serde_json::Value {
        serde_json::json!({"action":"add_track","name":"Approved lead","kind":"instrument"})
    }

    #[gpui::test]
    fn approval_is_visible_and_allows_exactly_one_edit(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::ReadOnly;
            this.panels.show(crate::dock::Panel::Agent);
            this.agent_permission(
                1,
                "edit_project".into(),
                serde_json::json!({"command":command()}),
            );
            assert!(this.agent_chat.controls.pending.is_some());
            assert!(this.agent_edit(command()).is_err());
        });
        crate::harness::paint(&app, cx);
        assert!(cx.debug_bounds("agent-approval").is_some());
        assert!(cx.debug_bounds("agent-allow-once").is_some());
        crate::harness::click("agent-allow-once", cx);
        app.update(cx, |this, _| {
            this.agent_edit(command()).unwrap();
            assert!(this.agent_edit(command()).is_err());
            assert!(
                this.project()
                    .tracks
                    .iter()
                    .any(|track| track.name == "Approved lead")
            );
            assert!(this.session.path().is_none());
        });
    }

    #[gpui::test]
    fn stale_or_denied_approvals_cannot_change_the_document(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, _| {
            this.settings.agent.policy = PolicyForTest::default();
            this.settings.agent.policy.mode = Mode::ReadOnly;
            this.agent_permission(
                2,
                "edit_project".into(),
                serde_json::json!({"command":command()}),
            );
            this.session
                .agent_command(serde_json::from_value(command()).unwrap())
                .unwrap();
            let before = this.project().clone();
            this.agent_approval(Approval::Once);
            assert!(this.agent_edit(command()).is_err());
            assert_eq!(this.project(), &before);
            this.agent_permission(
                3,
                "edit_project".into(),
                serde_json::json!({"command":command()}),
            );
            this.agent_approval(Approval::Once);
            this.settings.agent.policy.mode = Mode::Bypass;
            this.settings
                .agent
                .policy
                .set_rule("edit_project.*", Some(false))
                .unwrap();
            assert!(this.agent_edit(command()).is_err());
            assert_eq!(this.project(), &before);
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
}
