//! Conversation runtimes. Provider-specific messages never cross the worker boundary.

use crate::*;
use std::path::Path;
pub(super) mod antigravity;
pub(super) mod claude;
pub(super) mod codex;
pub(super) mod opencode;

fn native_project_directory(history: Option<&Path>) -> Option<&Path> {
    let history = history?;
    if history.file_name()? == ".auris-conversation.json" {
        return history.parent();
    }
    if history.file_name()? != "memory.json" {
        return None;
    }
    // Only project-local conversation stores identify a saved project. Recovery stores
    // remain private; unsaved conversations use the CLI's disposable working directory.
    let mut directory = history.parent()?.parent()?;
    for component in ["conversations", "agent", ".auris"] {
        if directory.file_name()? != component {
            return None;
        }
        directory = directory.parent()?;
    }
    Some(directory)
}

fn cli_preamble(prefs: &auris_session::AgentPreferences) -> String {
    let recovery = format!(
        "Tool errors are feedback, not task completion. After a recoverable argument or state error, inspect tool_help/current IDs, correct the call and continue the unfinished request. The configured allowance is up to {} failed attempts for the same tool and arguments; success resets that count. Failed calls do not consume the separate musical polishing budget. Preserve earlier successful edits, and never retry a permission refusal or cancellation.",
        prefs.tool_failure_limit()
    );
    format!(
        "{}\n{recovery}\nNative shell and file tools are available for the user's external-file and scripting tasks. Edit the open song through Auris live MCP tools so unsaved changes and undo stay coherent. Never rewrite its .auris file behind the window. External file writes and shell effects are durable and are not covered by Auris Undo. Request approvals when needed; never work around a refusal. Use explicit persistent paths for requested deliverables; temporary working files are removed when this conversation ends. Keep runtime discovery scoped to the project and dedicated runtime queries. For Python, prefer a project-local uv environment; Get-Command, uv python list --only-installed and uv python find locate executables without enumerating home or hidden configuration directories. Do not scan personal folders to find a runtime.",
        crate::system_prompt(prefs)
    )
}

/// Earlier conversation for a CLI worker that replaces an ephemeral conversation: a bounded
/// selection, not the transcript ([`memory::Memory::handoff`]).
fn memory_context(memory: &memory::Memory) -> String {
    memory.handoff()
}

/// The provider session a CLI backend resumes for this conversation, and what it is told first.
///
/// Only a saved project's own conversation is resumed: its folder is a stable working directory,
/// which Claude Code keys its sessions by, and its history is not a private recovery store.
pub(super) struct NativeResume {
    /// The backend's name in [`memory::Memory::native`].
    backend: &'static str,
    /// The session to resume, when one was saved.
    pub(super) previous: Option<String>,
    /// Whether the conversation may keep a provider session at all.
    pub(super) enabled: bool,
}

impl NativeResume {
    pub(super) fn new(backend: &'static str, memory: &memory::Memory, enabled: bool) -> Self {
        Self {
            backend,
            previous: enabled
                .then(|| {
                    memory
                        .native_thread(backend)
                        .map(|thread| thread.id.clone())
                })
                .flatten(),
            enabled,
        }
    }

    /// What the first request carries: only the missed turns when `resumed` is the saved
    /// session, otherwise the bounded selection of the whole conversation.
    pub(super) fn opening_context(&self, memory: &memory::Memory, resumed: Option<&str>) -> String {
        match (resumed, memory.native_thread(self.backend)) {
            (Some(id), Some(thread)) if thread.id == id => memory.delta_since(thread.seen),
            _ => memory_context(memory),
        }
    }

    /// Note, after a turn, that session `id` has seen the conversation so far.
    pub(super) fn record(&self, memory: &mut memory::Memory, id: Option<&str>) {
        match id {
            Some(id) if self.enabled => memory.record_native(self.backend, id),
            _ => memory.forget_native(self.backend),
        }
    }
}

/// A runtime owns the entire model/tool loop, not just a completion API client.
pub(crate) enum Backend {
    Rig(Options),
    Codex(auris_session::AgentPreferences),
    Claude(auris_session::AgentPreferences),
    OpenCode(auris_session::AgentPreferences),
    Antigravity(auris_session::AgentPreferences),
}

impl Backend {
    pub(crate) fn from_preferences(
        prefs: &auris_session::AgentPreferences,
    ) -> Result<Self, String> {
        match prefs.backend.as_str() {
            "" | "rig" => {
                let Command::Run(options) =
                    parse_command(&[], &|name| std::env::var(name).ok(), prefs)?
                else {
                    return Err("Expected agent settings".into());
                };
                Ok(Self::Rig(options))
            }
            "codex" => Ok(Self::Codex(prefs.clone())),
            "claude" => Ok(Self::Claude(prefs.clone())),
            "opencode" => Ok(Self::OpenCode(prefs.clone())),
            "antigravity" => Ok(Self::Antigravity(prefs.clone())),
            other => Err(format!("Unknown agent backend: {other}")),
        }
    }

    pub(crate) async fn run(
        self,
        bridge: &Bridge,
        history_path: Option<PathBuf>,
        fresh: bool,
        history: Option<HistorySnapshot>,
        cancelled: tokio::sync::oneshot::Receiver<()>,
    ) -> Result<(), String> {
        match self {
            Self::OpenCode(prefs) => {
                opencode::run(&prefs, bridge, history_path, fresh, history, cancelled).await
            }
            Self::Antigravity(prefs) => {
                antigravity::run(&prefs, bridge, history_path, fresh, history, cancelled).await
            }
            Self::Claude(prefs) => {
                claude::run(&prefs, bridge, history_path, fresh, history, cancelled).await
            }
            Self::Codex(prefs) => {
                codex::run(&prefs, bridge, history_path, fresh, history, cancelled).await
            }
            Self::Rig(mut options) => tokio::select! {
                biased;
                _ = cancelled => Ok(()),
                result = async {
                    let vision = runtime::preflight(&mut options).await?;
                    let agent = build_for_worker(&options, Some(bridge.clone()))?;
                    bridge.set_vision(vision);
                    json_conversation(&agent, &options, bridge, history_path, fresh, history).await
                } => result,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_project_directory_uses_the_project_outside_conversation_storage() {
        assert_eq!(
            native_project_directory(Some(Path::new(
                "Song/.auris/agent/conversations/id/memory.json"
            ))),
            Some(Path::new("Song"))
        );
    }

    #[test]
    fn native_project_directory_keeps_unsaved_recovery_storage_private() {
        assert_eq!(
            native_project_directory(Some(Path::new(
                "config/agent-recovery/workspace/id/memory.json"
            ))),
            None
        );
    }

    #[test]
    fn native_project_directory_supports_legacy_history_and_unsaved_scratch() {
        assert_eq!(
            native_project_directory(Some(Path::new("Song/.auris-conversation.json"))),
            Some(Path::new("Song"))
        );
        assert_eq!(native_project_directory(None), None);
    }
}
