//! Local issue drafts. Private context enters output only after per-report human consent.

use serde::{Deserialize, Serialize};

/// Stable kinds of private supporting evidence, each authorized independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextKind {
    /// Bounded application logs from this launch.
    UsageLogs,
    /// User-authored prompts from the selected conversation.
    CompositionPrompts,
    /// Locally recorded musical preference evidence.
    MusicPreferences,
}

impl ContextKind {
    /// The three independently reviewed categories.
    pub const ALL: [Self; 3] = [
        Self::UsageLogs,
        Self::CompositionPrompts,
        Self::MusicPreferences,
    ];

    /// Stable position in a report's consent controls.
    pub fn index(self) -> usize {
        match self {
            Self::UsageLogs => 0,
            Self::CompositionPrompts => 1,
            Self::MusicPreferences => 2,
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::UsageLogs => "Application logs (current launch)",
            Self::CompositionPrompts => "User prompts (selected conversation)",
            Self::MusicPreferences => "Local musical preference evidence",
        }
    }
}

/// A model may suggest a report, but cannot collect private context or grant consent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Suggestion {
    /// Short generic problem or feature title, without private quotes.
    pub title: String,
    /// Confirmed limitation or suspected defect; distinguish uncertainty and workarounds.
    pub explanation: String,
}

impl Suggestion {
    /// Bound model-authored metadata before it reaches a host's report invitation.
    pub fn validate(&self) -> Result<(), String> {
        if self.title.trim().is_empty()
            || self.title.chars().count() > 160
            || self.explanation.trim().is_empty()
            || self.explanation.chars().count() > 2000
        {
            return Err("Provide a nonempty title (up to 160 characters) and explanation (up to 2000 characters)".into());
        }
        Ok(())
    }
}

/// An ephemeral draft. Consent is deliberately neither serializable nor a saved preference.
#[derive(Default)]
pub struct Draft {
    /// User-editable title.
    pub title: String,
    /// User-editable symptoms, reproduction, expected and actual behavior.
    pub description: String,
    /// Basic build and audio metadata, supplied by the host without paths or song content.
    pub environment: String,
    approved: [Option<String>; 3],
    analysis: Option<String>,
}

impl Draft {
    /// Start a report without any private-context disclosure permission.
    pub fn new(title: String, description: String, environment: String) -> Self {
        Self {
            title,
            description,
            environment,
            ..Self::default()
        }
    }
    /// Include exactly the locally reviewed text after an explicit human action.
    /// This method must never be exposed as an agent operation.
    pub fn approve_context(&mut self, kind: ContextKind, reviewed_text: String) {
        self.approved[kind.index()] = Some(reviewed_text);
        self.analysis = None;
    }

    /// Remove supporting data and all analysis derived from the previous disclosure scope.
    pub fn revoke_context(&mut self, kind: ContextKind) {
        self.approved[kind.index()] = None;
        self.analysis = None;
    }

    /// Whether this report includes a reviewed, immutable snapshot of the category.
    pub fn includes(&self, kind: ContextKind) -> bool {
        self.approved[kind.index()].is_some()
    }

    /// Invalidate diagnostic output after edits or cancellation; source context stays local.
    pub fn clear_analysis(&mut self) {
        self.analysis = None;
    }

    /// Attach a bounded agent assessment of this exact draft, never a claim of reproduction.
    pub fn set_analysis(&mut self, text: &str) {
        self.analysis = Some(bounded(text, 12_000));
    }

    /// Render the same approved body for preview, clipboard and optional AI diagnosis.
    pub fn markdown(&self) -> String {
        let mut body = format!(
            "# {}\n\n{}\n\n## Environment\n\n{}\n",
            self.title.trim(),
            self.description.trim(),
            self.environment
        );
        for kind in ContextKind::ALL {
            if let Some(text) = &self.approved[kind.index()] {
                body.push_str(&format!("\n## {}\n\n", kind.heading()));
                // Private content is literal text, not active Markdown images or HTML.
                for line in text.lines() {
                    body.push_str("    ");
                    body.push_str(line);
                    body.push('\n');
                }
            }
        }
        if let Some(analysis) = &self.analysis {
            body.push_str("\n## AI assessment (not independently verified)\n\n");
            for line in analysis.lines() {
                body.push_str("    ");
                body.push_str(line);
                body.push('\n');
            }
        }
        body
    }
}

/// Keep previews finite and preserve Unicode boundaries, explicitly disclosing omissions.
pub fn bounded(text: &str, limit: usize) -> String {
    let mut chars = text.chars();
    let mut result: String = chars.by_ref().take(limit).collect();
    if chars.next().is_some() {
        result.push_str("\n[Additional content omitted]");
    }
    result
}

/// Best-effort local redaction before review; users must still inspect every included excerpt.
///
/// Paths are matched without regard to ASCII case, because the file systems Auris ships for do
/// not regard it either: `c:\users\alice` is the same folder as `C:\Users\Alice`, and so is
/// `/users/alice` on a default macOS volume. Other spellings of the same folder, such as a
/// Windows 8.3 short name, are the caller's to add to `private_paths`.
pub fn redact(text: &str, private_paths: &[String]) -> String {
    let mut text = bounded(text, 12_000);
    for path in private_paths.iter().filter(|path| !path.is_empty()) {
        for spelling in [
            path.clone(),
            path.replace('\\', "/"),
            path.replace('\\', "\\\\"),
        ] {
            text = replace_ignoring_ascii_case(&text, &spelling, "[local path]");
        }
    }
    text.lines()
        .map(|line| {
            let lower = line.to_ascii_lowercase();
            if [
                "authorization",
                "api_key",
                "api-key",
                "access_token",
                "password",
                "bearer ",
                "sk-",
            ]
            .iter()
            .any(|key| lower.contains(key))
            {
                "[Potential credential removed]"
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every occurrence of `needle` in `text`, compared without regard to ASCII case, replaced.
///
/// Only ASCII letters are folded, so a match is byte for byte as long as `needle` and starts and
/// ends on character boundaries: every byte that is not ASCII has to match exactly.
fn replace_ignoring_ascii_case(text: &str, needle: &str, replacement: &str) -> String {
    let (haystack, wanted) = (text.as_bytes(), needle.as_bytes());
    if wanted.is_empty() || wanted.len() > haystack.len() {
        return text.to_owned();
    }
    let mut result = String::with_capacity(text.len());
    let (mut copied, mut at) = (0, 0);
    while at + wanted.len() <= haystack.len() {
        if haystack[at..at + wanted.len()].eq_ignore_ascii_case(wanted) {
            result.push_str(&text[copied..at]);
            result.push_str(replacement);
            at += wanted.len();
            copied = at;
        } else {
            at += 1;
        }
    }
    result.push_str(&text[copied..]);
    result
}

/// Read only local music-choice evidence, away from the UI thread. No model request is made.
pub fn preference_preview() -> Result<String, String> {
    let preferences = crate::workshop::load_preferences()?;
    let mut result = String::from(
        "Recorded choices are contextual evidence, not a personality profile. Latest 12 choices only.\n",
    );
    for choice in preferences.selections.iter().rev().take(12) {
        if let Some(selected) = choice.alternatives.get(choice.chosen) {
            result.push_str(&format!(
                "\nRequest: {}\nSelected: {}\nDirection: {}\n",
                bounded(&choice.context, 400),
                bounded(&selected.title, 160),
                bounded(&selected.description, 400)
            ));
        }
    }
    Ok(bounded(&result, 12_000))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn issue_suggestion_is_nonmutating_and_cannot_grant_disclosure_permission() {
        use crate::{
            Session, SessionOptions,
            agent_policy::{Decision, Mode, Operation, Policy},
            live_agent::Command,
        };
        let args = serde_json::json!({"command":{"action":"suggest_issue_report","suggestion":{"title":"Missing support","explanation":"Verified limitation"}}});
        let operation = Operation::parse("edit_project", &args).unwrap();
        for mode in [Mode::ReadOnly, Mode::Plan, Mode::Bypass] {
            assert_eq!(
                Policy {
                    mode,
                    ..Default::default()
                }
                .decide(&operation),
                Decision::Allow
            );
        }
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        let revision = session.revision();
        let command: Command = serde_json::from_value(args["command"].clone()).unwrap();
        assert!(!command.is_mutating());
        let result = session.agent_command(command).unwrap();
        assert!(result.contains("\"private_context_included\":false"));
        assert_eq!(session.revision(), revision);
        assert_eq!(session.project(), &before);
        let mut forged = args["command"].clone();
        forged["approved_context"] = true.into();
        assert!(serde_json::from_value::<Command>(forged).is_err());
        assert!(
            Suggestion {
                title: String::new(),
                explanation: "x".into()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn no_private_context_is_included_until_that_category_is_approved() {
        let mut draft = Draft::default();
        assert!(!draft.markdown().contains("PRIVATE"));
        draft.approve_context(ContextKind::CompositionPrompts, "PRIVATE_PROMPT".into());
        assert!(draft.markdown().contains("PRIVATE_PROMPT"));
        assert!(!draft.includes(ContextKind::UsageLogs));
        draft.set_analysis("PRIVATE_DERIVED");
        draft.revoke_context(ContextKind::CompositionPrompts);
        assert!(!draft.markdown().contains("PRIVATE"));
        assert!(!Draft::default().includes(ContextKind::CompositionPrompts));
    }

    #[test]
    fn approval_binds_exact_text_and_private_markdown_stays_literal() {
        let mut draft = Draft::default();
        let mut source = "![private](https://example.com)\n# injected".to_string();
        draft.approve_context(ContextKind::UsageLogs, source.clone());
        source.push_str(" NEW_PRIVATE_DATA");
        assert!(!draft.markdown().contains("NEW_PRIVATE_DATA"));
        assert!(draft.markdown().contains("    ![private]"));
        draft.set_analysis("derived");
        draft.approve_context(ContextKind::MusicPreferences, "choice".into());
        assert!(!draft.markdown().contains("derived"));
    }

    #[test]
    fn redaction_handles_path_spellings_secrets_and_unicode_limits() {
        let preview = redact(
            "C:\\Users\\Alice\\song\nC:/Users/Alice/song\nAPI_KEY=secret\nあいう",
            &["C:\\Users\\Alice".into()],
        );
        assert!(!preview.contains("Alice"));
        assert!(!preview.contains("secret"));
        let preview = redact(
            "c:\\users\\alice\\song\nC:/USERS/ALICE/x\n/Users/alice/Music",
            &["C:\\Users\\Alice".into(), "/Users/Alice".into()],
        );
        assert!(!preview.to_ascii_lowercase().contains("alice"), "{preview}");
        assert_eq!(
            replace_ignoring_ascii_case("年 ÄBc äbc abc ABC", "abc", "_"),
            "年 ÄBc äbc _ _"
        );
        assert_eq!(
            bounded("あいうえ", 3),
            "あいう\n[Additional content omitted]"
        );
    }
}
