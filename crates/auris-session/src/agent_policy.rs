//! Permission decisions for the rig agent. MCP does not consult this policy.

use serde::{Deserialize, Serialize};

/// User-selected behavior for agent operations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Read freely; deny document changes and ask before network access unless allow-listed.
    ReadOnly,
    /// Allow ordinary document edits; ask before removal, replacement or network access.
    #[default]
    Edit,
    /// Investigate and propose a plan; document mutations are denied.
    Plan,
    /// Skip confirmations, while still enforcing deny rules and the live-document boundary.
    Bypass,
}

impl Mode {
    /// Stable name for commands and model context.
    pub fn name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Edit => "edit",
            Self::Plan => "plan",
            Self::Bypass => "bypass",
        }
    }

    /// Cycle ordinary modes. Bypass always requires an explicit selection.
    pub fn next(self) -> Self {
        match self {
            Self::ReadOnly => Self::Edit,
            Self::Edit => Self::Plan,
            Self::Plan | Self::Bypass => Self::ReadOnly,
        }
    }
}

/// Saved permission rules. Exact operation names, `edit_project.*`, or `*` are accepted.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Policy {
    /// The selected permission mode.
    pub mode: Mode,
    /// Operations that may run without confirmation, except mutations in read-only and plan mode.
    pub allow: Vec<String>,
    /// Operations that are denied in every mode, including bypass.
    pub deny: Vec<String>,
}

/// The permission decision before an operation runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Decision {
    /// Execute without a confirmation.
    Allow,
    /// Wait for an explicit user answer.
    Ask,
    /// Refuse and explain the constraint to the model.
    Deny(String),
}

/// The classified, validated operation behind a tool call.
#[derive(Clone, Debug)]
pub struct Operation {
    /// Exact rule name, such as `edit_project.remove_track`.
    pub name: String,
    /// Whether it changes the document.
    pub mutating: bool,
    /// Whether it removes existing content, replaces the arrangement, or uses the network.
    pub confirm: bool,
}

/// Every operation exposed by the live agent, for rule editing and validation.
pub const OPERATIONS: &[&str] = &[
    "edit_project.inspect",
    "edit_project.inspect_audio",
    "edit_project.read_notes",
    "edit_project.compose",
    "edit_project.add_track",
    "edit_project.rename_track",
    "edit_project.remove_track",
    "edit_project.set_instrument",
    "edit_project.set_level",
    "edit_project.set_track_state",
    "edit_project.add_clip",
    "edit_project.add_note",
    "edit_project.add_notes",
    "edit_project.replace_notes",
    "edit_project.set_tempo",
    "edit_project.set_loop",
    "edit_project.remove_notes",
    "list_instruments",
    "search_instruments",
    "instrument_diagnostics",
    "similar_instruments",
    "list_presets",
    "list_progressions",
    "spec_reference",
    "search_documentation",
    "search_internet",
];

impl Operation {
    /// Validate the command before assigning permissions; unknown tools fail closed.
    pub fn parse(tool: &str, args: &serde_json::Value) -> Result<Self, String> {
        if tool == "edit_project" {
            let object = args.as_object().ok_or("Expected tool arguments")?;
            let command = object.get("command").ok_or("missing field `command`")?;
            if object.len() != 1 {
                return Err("Expected only the command field".into());
            }
            let parsed: crate::live_agent::Command =
                serde_json::from_value(command.clone()).map_err(|e| e.to_string())?;
            let action = command["action"].as_str().ok_or("Missing action")?;
            let mutating = !matches!(
                parsed,
                crate::live_agent::Command::Inspect {}
                    | crate::live_agent::Command::InspectAudio { .. }
                    | crate::live_agent::Command::ReadNotes { .. }
                    | crate::live_agent::Command::ListInstruments { .. }
                    | crate::live_agent::Command::SearchInstruments { .. }
                    | crate::live_agent::Command::InstrumentDiagnostics { .. }
                    | crate::live_agent::Command::SimilarInstruments { .. }
            );
            let confirm = matches!(
                parsed,
                crate::live_agent::Command::RemoveTrack { .. }
                    | crate::live_agent::Command::RemoveNotes { .. }
                    | crate::live_agent::Command::ReplaceNotes { .. }
                    | crate::live_agent::Command::Compose { replace: true, .. }
            );
            Ok(Self {
                name: if matches!(
                    action,
                    "list_instruments"
                        | "search_instruments"
                        | "instrument_diagnostics"
                        | "similar_instruments"
                ) {
                    action.into()
                } else {
                    format!("edit_project.{action}")
                },
                mutating,
                confirm,
            })
        } else if !tool.contains('.') && OPERATIONS.contains(&tool) {
            Ok(Self {
                name: tool.into(),
                mutating: false,
                confirm: tool == "search_internet",
            })
        } else {
            Err(format!("Tool '{tool}' is not available to the live agent"))
        }
    }
}

impl Policy {
    /// Enforce deny rules first, then the mode, allow rules, and ordinary defaults.
    pub fn decide(&self, operation: &Operation) -> Decision {
        let matches = |rules: &[String]| {
            rules.iter().any(|rule| {
                rule == "*"
                    || rule == &operation.name
                    || (rule == "edit_project.*" && operation.name.starts_with("edit_project."))
            })
        };
        if matches(&self.deny) {
            return Decision::Deny(format!(
                "{} is prohibited by the user's deny list. Do not retry it.",
                operation.name
            ));
        }
        if self.mode == Mode::ReadOnly && operation.mutating {
            return Decision::Deny("Read-only mode does not permit document changes. Switch to Edit mode before requesting this operation.".into());
        }
        if self.mode == Mode::Plan && operation.mutating {
            return Decision::Deny("Plan mode does not permit document changes. Inspect and present a plan; the user must switch modes before execution.".into());
        }
        if self.mode == Mode::Bypass
            || matches(&self.allow)
            || (!operation.mutating && !operation.confirm)
            || (self.mode == Mode::Edit && operation.mutating && !operation.confirm)
        {
            Decision::Allow
        } else {
            Decision::Ask
        }
    }

    /// Set one exact operation to default, allowed or denied. Removes the opposite exact rule.
    pub fn set_rule(&mut self, operation: &str, allow: Option<bool>) -> Result<(), String> {
        if !OPERATIONS.contains(&operation) && !["*", "edit_project.*"].contains(&operation) {
            return Err("Unknown operation name".into());
        }
        self.allow.retain(|name| name != operation);
        self.deny.retain(|name| name != operation);
        match allow {
            Some(true) => self.allow.push(operation.into()),
            Some(false) => self.deny.push(operation.into()),
            None => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit() -> Operation {
        Operation::parse("edit_project", &serde_json::json!({"command":{"action":"add_track","name":"Lead","kind":"instrument"}})).unwrap()
    }

    #[test]
    fn modes_and_rules_have_explicit_precedence() {
        let mut policy = Policy::default();
        assert_eq!(policy.decide(&edit()), Decision::Allow);
        policy.mode = Mode::ReadOnly;
        assert!(matches!(policy.decide(&edit()), Decision::Deny(_)));
        policy.set_rule("edit_project.*", Some(true)).unwrap();
        assert!(matches!(policy.decide(&edit()), Decision::Deny(_)));
        policy.mode = Mode::Plan;
        assert!(matches!(policy.decide(&edit()), Decision::Deny(_)));
        policy.mode = Mode::Bypass;
        policy
            .set_rule("edit_project.add_track", Some(false))
            .unwrap();
        assert!(matches!(policy.decide(&edit()), Decision::Deny(_)));
    }

    #[test]
    fn read_only_denies_allow_listed_document_changes() {
        let mut policy = Policy {
            mode: Mode::ReadOnly,
            ..Policy::default()
        };
        policy.set_rule("edit_project.*", Some(true)).unwrap();

        assert!(matches!(policy.decide(&edit()), Decision::Deny(_)));
    }

    #[test]
    fn read_only_keeps_network_confirmation_and_exact_allow_rules() {
        let operation = Operation::parse(
            "search_internet",
            &serde_json::json!({"query":"music theory"}),
        )
        .unwrap();
        let mut policy = Policy {
            mode: Mode::ReadOnly,
            ..Policy::default()
        };
        assert_eq!(policy.decide(&operation), Decision::Ask);

        policy.set_rule("search_internet", Some(true)).unwrap();

        assert_eq!(policy.decide(&operation), Decision::Allow);
    }

    #[test]
    fn removal_and_network_require_confirmation_in_edit_mode() {
        for (tool, args) in [
            (
                "edit_project",
                serde_json::json!({"command":{"action":"remove_track","track":1}}),
            ),
            (
                "edit_project",
                serde_json::json!({"command":{"action":"compose","preset":"game-loop","replace":true}}),
            ),
            ("search_internet", serde_json::json!({"query":"music"})),
        ] {
            assert_eq!(
                Policy::default().decide(&Operation::parse(tool, &args).unwrap()),
                Decision::Ask
            );
        }
        assert!(Operation::parse("render", &serde_json::json!({})).is_err());
        assert!(Operation::parse("edit_project.remove_track", &serde_json::json!({})).is_err());
    }

    #[test]
    fn instrument_diagnostics_has_an_exact_user_editable_rule() {
        let operation = Operation::parse(
            "edit_project",
            &serde_json::json!({"command":{"action":"instrument_diagnostics","offset":0,"limit":20}}),
        )
        .unwrap();
        assert_eq!(operation.name, "instrument_diagnostics");
        assert!(!operation.mutating);

        let mut policy = Policy::default();
        policy
            .set_rule("instrument_diagnostics", Some(false))
            .unwrap();
        assert!(matches!(policy.decide(&operation), Decision::Deny(_)));
    }
}
