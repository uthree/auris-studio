//! Permission decisions and controls owned by the desktop session.
use super::*;
use auris_session::agent_policy::{Decision, Mode, OPERATIONS, Operation};

#[derive(Default)]
pub(super) struct Controls {
    pub(super) pending: Option<Pending>,
    pub(super) permits: Vec<(serde_json::Value, u64)>,
    pub(super) rules_open: bool,
    pub(super) compacting: bool,
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
        self.save_agent_policy();
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
        let mut row = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .p_1()
            .border_b_1()
            .border_color(theme.border);
        for mode in [Mode::ReadOnly, Mode::Edit, Mode::Plan, Mode::Bypass] {
            row = row.child(button(
                SharedString::from(format!("agent-mode-{}", mode.name())),
                self.t(mode_key(mode)),
                ButtonStyle::Normal,
                self.settings.agent.policy.mode == mode,
                if mode == Mode::Bypass {
                    theme.warning
                } else {
                    theme.accent
                },
                theme,
                cx.listener(move |this, _, _, cx| {
                    this.agent_mode(mode);
                    cx.notify();
                }),
            ));
        }
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
        row.into_any_element()
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
}
