//! Launch and isolate ACP agent processes.
//!
//! This module deliberately owns only process discovery and environment hygiene. ACP
//! initialization, session creation, and capability negotiation belong to the transport
//! implementation so they can evolve independently of executable lookup.

use crate::live_mcp::LiveMcp;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::process::Command;

/// A prepared ACP subprocess and the metadata needed by the session layer.
pub(crate) struct Launch {
    /// Child command, configured for piped JSON-RPC stdio.
    pub command: Command,
    /// Adapter extension options sent in session/new._meta.
    pub session_meta: Value,
    /// Whether the adapter accepts the live server through session/new.
    pub mcp_in_session: bool,
    /// Instructions supplied through the advertised client system prompt extension.
    pub system_prompt: Option<String>,
}

fn executable_from_env_or_path(env_name: &str, names: &[&str]) -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os(env_name).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if !path.is_file() {
            return Err(format!(
                "ACP executable from {env_name} was not found: {}",
                path.display()
            ));
        }
        return Ok(path);
    }
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        for name in names {
            let path = directory.join(format!("{name}{suffix}"));
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err(format!(
        "ACP adapter was not found. Install one of [{}], or set {env_name} to its native executable",
        names.join(", ")
    ))
}

/// Resolve an installed npm adapter without invoking npm or downloading anything. On Windows
/// npm's command shims are `.cmd` files; run the package's JS entry through `node.exe` instead.
fn adapter_entry(
    env_name: &str,
    package_names: &[&str],
    command_names: &[&str],
) -> Result<(PathBuf, Vec<String>), String> {
    if let Some(value) = std::env::var_os(env_name).filter(|value| !value.is_empty()) {
        let path = PathBuf::from(value);
        if !path.is_file() {
            return Err(format!(
                "ACP adapter from {env_name} was not found: {}",
                path.display()
            ));
        }
        let is_js = path.extension().is_some_and(|ext| {
            ["js", "mjs", "cjs"]
                .iter()
                .any(|suffix| ext.eq_ignore_ascii_case(suffix))
        });
        if is_js {
            let node = executable_from_env_or_path("AURIS_NODE", &["node"])?;
            return Ok((node, vec![path.to_string_lossy().into_owned()]));
        }
        if cfg!(windows)
            && path.extension().is_some_and(|ext| {
                ["cmd", "bat", "ps1"]
                    .iter()
                    .any(|suffix| ext.eq_ignore_ascii_case(suffix))
            })
        {
            return Err(format!(
                "Set {env_name} to the adapter's JavaScript entry or native executable, not a shell shim"
            ));
        }
        return Ok((path, Vec::new()));
    }
    let managed = match env_name {
        "AURIS_CODEX_ACP" => Some(crate::AcpAdapter::Codex),
        "AURIS_CLAUDE_ACP" => Some(crate::AcpAdapter::Claude),
        _ => None,
    };
    if let Some(entry) = managed.and_then(crate::adapter_setup::managed_entry) {
        return Ok(entry);
    }
    if let Ok(path) = executable_from_env_or_path("AURIS_NODE", &["node"]) {
        let mut roots = Vec::new();
        if let Some(appdata) = std::env::var_os("APPDATA") {
            roots.push(PathBuf::from(appdata).join("npm/node_modules"));
        }
        if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
            roots.push(PathBuf::from(home).join(".npm-global/lib/node_modules"));
        }
        roots.extend([
            PathBuf::from("/usr/local/lib/node_modules"),
            PathBuf::from("/usr/lib/node_modules"),
        ]);
        for root in roots {
            for package in package_names {
                for entry in ["dist/index.js", "dist/main.js", "bin/acp.js"] {
                    let candidate = root.join(package).join(entry);
                    if candidate.is_file() {
                        return Ok((path, vec![candidate.to_string_lossy().into_owned()]));
                    }
                }
            }
        }
    }
    let path = executable_from_env_or_path(env_name, command_names)?;
    Ok((path, Vec::new()))
}

fn isolated_command(executable: PathBuf, cwd: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Never inherit a parent agent's routing, MCP token, or provider runtime settings.
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper.starts_with("AURIS_")
            || (upper.starts_with("CODEX_") && upper != "CODEX_HOME")
            || upper.starts_with("OPENAI_")
            || upper.starts_with("CLAUDE_AGENT_SDK_")
            || upper == "CLAUDECODE"
        {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW
    command
}

/// Run a read-only CLI query without exposing configuration or credentials in logs.
async fn query(path: &Path, cwd: &Path, args: &[&str]) -> Result<std::process::Output, String> {
    let mut command = isolated_command(path.to_owned(), cwd);
    command.args(args).stdin(Stdio::null());
    tokio::time::timeout(Duration::from_secs(10), command.output())
        .await
        .map_err(|_| "Agent configuration query timed out".to_string())?
        .map_err(|error| format!("Cannot query installed agent: {error}"))
}

fn codex_config(
    prefs: &auris_session::AgentPreferences,
    native: bool,
    inherited: &Value,
    mcp: Option<&LiveMcp>,
) -> Result<Value, String> {
    let entries = inherited
        .as_array()
        .ok_or("Codex did not return a valid MCP configuration list")?;
    let mut servers = serde_json::Map::new();
    for entry in entries {
        let name = entry["name"]
            .as_str()
            .ok_or("Codex MCP configuration omitted a server name")?;
        // Thread config replaces each server table. Even disabled entries need a valid
        // transport shape; never copy inherited commands, environment or credentials.
        servers.insert(
            name.into(),
            json!({"enabled":false,"command":"auris-disabled-mcp"}),
        );
    }
    if let Some(mcp) = mcp {
        servers.insert(
            "auris_live".into(),
            json!({"enabled":true,"required":true,"url":mcp.url,
            "bearer_token_env_var":"AURIS_LIVE_MCP_TOKEN","default_tools_approval_mode":"approve"}),
        );
    }
    Ok(
        json!({"model_provider":"openai","forced_login_method":"chatgpt","approvals_reviewer":"user",
        "features":{"shell_snapshot":false,"hooks":false,"plugins":false,"apps":false,"multi_agent":false,
            "code_mode":false,"remote_plugin":false,"memories":false,"skip_host_skill_discovery":true,
            "skill_mcp_dependency_install":false,"shell_tool":native,"unified_exec":native,"apply_patch_freeform":native,
            "fast_mode":true},
        "web_search":"disabled","project_doc_max_bytes":0,"mcp_servers":servers,
        "service_tier":if prefs.fast_mode {"fast"} else {"default"}}),
    )
}

fn claude_meta(prefs: &auris_session::AgentPreferences, native: bool) -> Value {
    let tools = if native {
        vec![
            "Bash",
            "Read",
            "Glob",
            "Grep",
            "Write",
            "Edit",
            "NotebookEdit",
        ]
    } else {
        vec![]
    };
    json!({"claudeCode":{"options":{
        "settingSources":[],"tools":tools,"mcpServers":{},"plugins":[],"persistSession":false,
        "allowDangerouslySkipPermissions":false,"promptSuggestions":false,
        "extraArgs":{"strict-mcp-config":null,"disable-slash-commands":null,"no-chrome":null},
        "settings":{"disableAllHooks":true,"autoMemoryEnabled":false,"enabledPlugins":{},"fastMode":prefs.fast_mode,
            "permissions":{"defaultMode":"default","allow":["mcp__auris_live__*"],"ask":tools}}
    }}})
}

/// Prepare only installed components. Configuration changes are scoped to this child.
pub(crate) async fn prepare(
    prefs: &auris_session::AgentPreferences,
    cwd: &Path,
    native: bool,
    mcp: Option<&LiveMcp>,
    project: Option<&Path>,
) -> Result<Launch, String> {
    if prefs.backend == "opencode" {
        let version = query(&super::opencode::executable(prefs)?, cwd, &["--version"]).await?;
        if !version.status.success() {
            return Err("OpenCode version check failed".into());
        }
        let major = super::opencode::cli_major(&String::from_utf8_lossy(&version.stdout))?;
        return Ok(Launch {
            command: super::opencode::command(prefs, cwd, native, major, mcp)?,
            session_meta: json!({}),
            mcp_in_session: major == 1,
            system_prompt: None,
        });
    }
    let instructions = if native {
        super::cli_preamble(prefs)
    } else {
        crate::approval_review::INSTRUCTIONS.into()
    };
    match prefs.backend.as_str() {
        "codex" => {
            let cli = super::codex::executable(prefs)?;
            let login = query(&cli, cwd, &["login", "status"]).await?;
            let status = format!(
                "{}{}",
                String::from_utf8_lossy(&login.stdout),
                String::from_utf8_lossy(&login.stderr)
            );
            if !login.status.success() || !status.contains("ChatGPT") {
                return Err("Sign in to Codex with a ChatGPT account using codex login. Use the API backend for API keys".into());
            }
            let listing = query(&cli, project.unwrap_or(cwd), &["mcp", "list", "--json"]).await?;
            if !listing.status.success() {
                return Err(
                    "Cannot inspect Codex MCP configuration for an isolated ACP session".into(),
                );
            }
            let inherited: Value = serde_json::from_slice(&listing.stdout)
                .map_err(|_| "Codex returned invalid MCP configuration JSON")?;
            let config = codex_config(prefs, native, &inherited, mcp)?;
            let (path, args) = adapter_entry(
                "AURIS_CODEX_ACP",
                &["@agentclientprotocol/codex-acp"],
                &["codex-acp"],
            )?;
            let mut command = isolated_command(path, cwd);
            command
                .args(args)
                .env("CODEX_PATH", cli)
                .env("CODEX_CONFIG", config.to_string())
                .env("MODEL_PROVIDER", "openai")
                .env_remove("DEFAULT_AUTH_REQUEST")
                .env("INITIAL_AGENT_MODE", "read-only");
            if let Some(mcp) = mcp {
                command.env("AURIS_LIVE_MCP_TOKEN", &mcp.token);
            }
            // Keep the disable entries in CODEX_CONFIG: the adapter replaces that map
            // when session/new supplies MCP servers, and Codex otherwise merges user config.
            Ok(Launch {
                command,
                session_meta: json!({}),
                mcp_in_session: false,
                system_prompt: Some(instructions),
            })
        }
        "claude" => {
            let cli = super::claude::executable(prefs)?;
            let (path, args) = adapter_entry(
                "AURIS_CLAUDE_ACP",
                &["@agentclientprotocol/claude-agent-acp"],
                &["claude-agent-acp"],
            )?;
            let mut command = isolated_command(path, cwd);
            command
                .args(args)
                .env("CLAUDE_CODE_EXECUTABLE", cli)
                .env_remove("CLAUDE_CODE_SUBAGENT_MODEL")
                .env_remove("CLAUDE_MODEL_CONFIG")
                .env("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION", "false");
            Ok(Launch {
                command,
                session_meta: claude_meta(prefs, native),
                mcp_in_session: true,
                system_prompt: Some(instructions),
            })
        }
        other => Err(format!("ACP is not available for backend: {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn codex_disables_every_inherited_mcp_and_reviewer_tools() {
        let prefs = auris_session::AgentPreferences::default();
        let config = codex_config(
            &prefs,
            false,
            &json!([{"name":"external.with.dot"},{"name":"auris_live"}]),
            None,
        )
        .unwrap();
        assert_eq!(config["mcp_servers"]["external.with.dot"]["enabled"], false);
        assert_eq!(config["mcp_servers"]["auris_live"]["enabled"], false);
        assert_eq!(config["features"]["shell_tool"], false);
        assert_eq!(config["features"]["apply_patch_freeform"], false);
        assert_eq!(config["forced_login_method"], "chatgpt");
        assert_eq!(config["service_tier"], "default");
    }
    #[test]
    fn claude_isolates_settings_and_requires_native_tool_approval() {
        let prefs = auris_session::AgentPreferences::default();
        let live = claude_meta(&prefs, true);
        let options = &live["claudeCode"]["options"];
        assert_eq!(options["settingSources"], json!([]));
        assert_eq!(options["allowDangerouslySkipPermissions"], false);
        assert_eq!(options["settings"]["permissions"]["ask"], options["tools"]);
        assert_eq!(
            claude_meta(&prefs, false)["claudeCode"]["options"]["tools"],
            json!([])
        );
    }
}
