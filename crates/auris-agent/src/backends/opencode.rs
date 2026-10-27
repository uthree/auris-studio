//! OpenCode executable discovery and isolated configuration for the common ACP backend.

use crate::live_mcp::LiveMcp;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};
use tokio::process::Command;

const NATIVE_TOOLS: &[&str] = &[
    "bash",
    "shell",
    "read",
    "glob",
    "grep",
    "list",
    "write",
    "edit",
    "patch",
    "apply_patch",
];

pub(crate) fn executable(prefs: &auris_session::AgentPreferences) -> Result<PathBuf, String> {
    if !prefs.opencode_path.trim().is_empty() {
        let path = PathBuf::from(prefs.opencode_path.trim());
        if cfg!(windows)
            && path
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
        {
            return Err(
                "Choose the native opencode.exe executable, not a .cmd or .ps1 launcher".into(),
            );
        }
        return Ok(path);
    }
    let name = if cfg!(windows) {
        "opencode.exe"
    } else {
        "opencode"
    };
    let mut directories: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if cfg!(target_os = "macos") {
        directories.extend([
            PathBuf::from("/opt/homebrew/bin"),
            PathBuf::from("/usr/local/bin"),
        ]);
    }
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        let home = PathBuf::from(home);
        directories.extend([home.join(".opencode/bin"), home.join(".local/bin")]);
    }
    directories.into_iter().map(|dir| dir.join(name)).find(|path| path.is_file())
        .ok_or_else(|| "OpenCode CLI was not found. Install OpenCode, run opencode auth login, or set its executable path in Agent settings".into())
}

pub(crate) fn cli_major(version: &str) -> Result<u32, String> {
    let number = version
        .trim()
        .strip_prefix("opencode ")
        .unwrap_or(version.trim())
        .trim_start_matches('v');
    match number.split('.').next() {
        Some("1") => Ok(1),
        Some("2") => Ok(2),
        _ => Err(format!(
            "Unsupported OpenCode version: {} (expected 1.x or 2.x)",
            version.trim()
        )),
    }
}

#[cfg(test)]
fn working_directory() -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("auris-opencode-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().map_err(|error| error.to_string())
}

fn opencode_preamble(prefs: &auris_session::AgentPreferences) -> String {
    let shell = if cfg!(windows) {
        "\nThe native shell uses cmd.exe. Invoke powershell.exe -NoProfile -Command explicitly when PowerShell syntax is needed."
    } else {
        ""
    };
    format!(
        "{}{shell}\nOpenCode exposes the live tools as auris_live_<tool name>, for example auris_live_inspect_project. Call them directly. Do not use native shell commands or directory scans to discover Auris tools. If the live tools are unavailable, report the connection problem instead.",
        super::cli_preamble(prefs)
    )
}

fn configuration(prefs: &auris_session::AgentPreferences, native: bool, major: u32) -> Value {
    if major == 2 {
        let mut permissions = vec![json!({"action":"*","resource":"*","effect":"deny"})];
        if native {
            for action in [
                "shell",
                "read",
                "glob",
                "grep",
                "edit",
                "external_directory",
            ] {
                permissions.push(json!({"action":action,"resource":"*","effect":"ask"}));
            }
            permissions.push(json!({"action":"auris_live_*","resource":"*","effect":"allow"}));
        }
        return json!({"$schema":"https://opencode.ai/config.json", "update":"disable",
            "share":"disabled", "snapshots":false, "lsp":false, "formatter":false,
            "default_agent":"auris", "permissions":permissions,
            "agents":{"auris":{"mode":"primary", "system":if native { opencode_preamble(prefs) } else { crate::approval_review::INSTRUCTIONS.into() },
                "permissions":permissions}}});
    }
    let mut tools = json!({"*":false});
    let mut permission = json!({"*":"deny"});
    if native {
        for tool in NATIVE_TOOLS {
            tools[tool] = json!(true);
            permission[tool] = json!("ask");
        }
        tools["auris_live_*"] = json!(true);
        // ToolHost still authorizes each exact live call through the desktop.
        permission["auris_live_*"] = json!("allow");
        permission["external_directory"] = json!("ask");
    }
    json!({"$schema":"https://opencode.ai/config.json", "autoupdate":false,
        "share":"disabled", "snapshot":false, "lsp":false, "formatter":false,
        "default_agent":"auris", "permission":permission, "tools":tools,
        "agent":{"auris":{"mode":"primary", "prompt":if native { opencode_preamble(prefs) } else { crate::approval_review::INSTRUCTIONS.into() },
            "permission":permission, "tools":tools}}})
}

pub(crate) fn command(
    prefs: &auris_session::AgentPreferences,
    cwd: &Path,
    native: bool,
    major: u32,
    mcp: Option<&LiveMcp>,
) -> Result<Command, String> {
    // Keep XDG_DATA_HOME untouched: OpenCode can use its own authentication store. An
    // isolated config home excludes inherited MCP servers, custom tools and user plugins.
    let config_home = cwd.join("config");
    std::fs::create_dir_all(&config_home).map_err(|error| error.to_string())?;
    let mut file = tempfile::NamedTempFile::new_in(cwd).map_err(|error| error.to_string())?;
    let mut config = configuration(prefs, native, major);
    if cfg!(windows) {
        // OpenCode 1.19's PowerShell scanner misses bare .NET method expressions and
        // executes them without requesting consent. cmd parses these as commands;
        // PowerShell remains available through an explicit, permission-checked invocation.
        config["shell"] = json!("cmd.exe");
    }
    if major == 2
        && let Some(mcp) = mcp
    {
        // ACP-added V2 servers always enable Code Mode. Configure the server here
        // to retain direct tool names; only the child environment carries its token.
        config["mcp"] = json!({"servers":{"auris_live":{"type":"remote","url":mcp.url,
            "oauth":false,"codemode":false,
            "headers":{"Authorization":"Bearer {env:AURIS_OPENCODE_MCP_TOKEN}"}}}});
    }
    std::io::Write::write_all(&mut file, config.to_string().as_bytes())
        .map_err(|error| error.to_string())?;
    let config_path = cwd.join("opencode.json");
    file.persist_noclobber(&config_path)
        .map_err(|error| error.to_string())?;
    let mut command = Command::new(executable(prefs)?);
    command
        .arg("acp")
        .current_dir(cwd)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("OPENCODE_CONFIG", config_path)
        .env("OPENCODE_CONFIG_DIR", config_home.join("opencode"))
        // OpenCode also scans ~/.opencode independently of XDG_CONFIG_HOME.
        .env("OPENCODE_TEST_HOME", cwd)
        .env(
            "OPENCODE_PERMISSION",
            configuration(prefs, native, 1)["permission"].to_string(),
        )
        .env("OPENCODE_DISABLE_PROJECT_CONFIG", "true")
        .env("OPENCODE_CONFIG_PROJECT_DISABLE", "true")
        .env("OPENCODE_DISABLE_AUTOUPDATE", "true")
        .env("OPENCODE_DISABLE_EXTERNAL_SKILLS", "true")
        .env("OPENCODE_DISABLE_CLAUDE_CODE", "true")
        .env("OPENCODE_AUTO_SHARE", "false")
        .env_remove("OPENCODE_CONFIG_CONTENT")
        .env_remove("OPENCODE_CLI_CONFIG_CONTENT")
        .env_remove("OPENCODE_TEST_MANAGED_CONFIG_DIR")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if major == 2
        && let Some(mcp) = mcp
    {
        command.env("AURIS_OPENCODE_MCP_TOKEN", &mcp.token);
    } else {
        command.env_remove("AURIS_OPENCODE_MCP_TOKEN");
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::ToolHost;
    use std::collections::HashMap;
    #[test]
    fn configuration_is_private_and_native_launchers_are_required_on_windows() {
        let cwd = working_directory().unwrap();
        let prefs = auris_session::AgentPreferences {
            opencode_path: if cfg!(windows) {
                "C:/Tools/opencode.exe"
            } else {
                "/usr/local/bin/opencode"
            }
            .into(),
            ..Default::default()
        };
        let command = command(&prefs, cwd.path(), true, 1, None).unwrap();
        let args: Vec<_> = command.as_std().get_args().collect();
        assert_eq!(args, ["acp"]);
        let env: HashMap<_, _> = command.as_std().get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("XDG_CONFIG_HOME"))
                .unwrap()
                .unwrap(),
            cwd.path().join("config").as_os_str()
        );
        assert!(!env.contains_key(std::ffi::OsStr::new("XDG_DATA_HOME")));
        assert_eq!(
            env.get(std::ffi::OsStr::new("OPENCODE_CONFIG_CONTENT")),
            Some(&None)
        );
        let config: Value =
            serde_json::from_slice(&std::fs::read(cwd.path().join("opencode.json")).unwrap())
                .unwrap();
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(config["permission"]["auris_live_*"], "allow");
        assert_eq!(config["permission"]["*"], "deny");
        if cfg!(windows) {
            assert_eq!(config["shell"], "cmd.exe");
        }
        assert_eq!(config["tools"]["*"], false);
        let review = configuration(&auris_session::AgentPreferences::default(), false, 1);
        assert_eq!(review["permission"], json!({"*":"deny"}));
        assert_eq!(review["tools"], json!({"*":false}));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for path in [cwd.path().to_path_buf(), cwd.path().join("opencode.json")] {
                assert_eq!(
                    std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
                    0
                );
            }
        }
        let launcher = auris_session::AgentPreferences {
            opencode_path: "opencode.cmd".into(),
            ..Default::default()
        };
        assert_eq!(executable(&launcher).is_err(), cfg!(windows));
        assert!(matches!(
            super::super::Backend::from_preferences(&auris_session::AgentPreferences {
                backend: "opencode".into(),
                ..Default::default()
            })
            .unwrap(),
            super::super::Backend::Acp(_)
        ));
    }

    #[test]
    fn custom_system_prompt_does_not_override_review_instructions() {
        let prefs = auris_session::AgentPreferences {
            system_prompt: "Keep the motif.\n二番も作る。".into(),
            ..Default::default()
        };
        for major in [1, 2] {
            let (agents, prompt) = if major == 1 {
                ("agent", "prompt")
            } else {
                ("agents", "system")
            };
            let native = configuration(&prefs, true, major);
            assert!(
                native[agents]["auris"][prompt]
                    .as_str()
                    .unwrap()
                    .starts_with(&prefs.system_prompt)
            );
            let review = configuration(&prefs, false, major);
            assert_eq!(
                review[agents]["auris"][prompt],
                crate::approval_review::INSTRUCTIONS
            );
        }
    }

    #[test]
    fn version_two_uses_current_agent_and_permission_fields() {
        assert_eq!(cli_major("2.1.3\n").unwrap(), 2);
        assert_eq!(cli_major("opencode v2.1.3\n").unwrap(), 2);
        assert_eq!(cli_major("1.19.3").unwrap(), 1);
        assert!(cli_major("3.0.0").is_err());
        assert!(cli_major("not a version").is_err());
        let config = configuration(&auris_session::AgentPreferences::default(), true, 2);
        assert!(
            config["agents"]["auris"]["system"]
                .as_str()
                .unwrap()
                .contains("Auris")
        );
        assert!(config.get("agent").is_none());
        assert!(config.get("tools").is_none());
        let rules = config["permissions"].as_array().unwrap();
        assert_eq!(
            rules[0],
            json!({"action":"*","resource":"*","effect":"deny"})
        );
        assert!(rules.contains(&json!({"action":"shell","resource":"*","effect":"ask"})));
        assert!(rules.contains(&json!({"action":"auris_live_*","resource":"*","effect":"allow"})));
        assert_eq!(
            config["agents"]["auris"]["permissions"],
            config["permissions"]
        );
        let review = configuration(&auris_session::AgentPreferences::default(), false, 2);
        assert_eq!(
            review["permissions"],
            json!([{ "action":"*","resource":"*","effect":"deny" }])
        );
        assert_eq!(
            review["agents"]["auris"]["permissions"],
            review["permissions"]
        );
    }

    #[tokio::test]
    async fn version_two_exposes_direct_mcp_tools_and_keeps_token_out_of_config() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
        let cwd = working_directory().unwrap();
        let prefs = auris_session::AgentPreferences {
            opencode_path: if cfg!(windows) {
                "C:/Tools/opencode.exe"
            } else {
                "/usr/local/bin/opencode"
            }
            .into(),
            ..Default::default()
        };
        let command = command(&prefs, cwd.path(), true, 2, Some(&mcp)).unwrap();
        let bytes = std::fs::read(cwd.path().join("opencode.json")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains(&mcp.token));
        let config: Value = serde_json::from_slice(&bytes).unwrap();
        let server = &config["mcp"]["servers"]["auris_live"];
        assert_eq!(server["codemode"], false);
        assert_eq!(server["oauth"], false);
        assert_eq!(server["url"], mcp.url);
        let env: HashMap<_, _> = command.as_std().get_envs().collect();
        assert_eq!(
            env.get(std::ffi::OsStr::new("AURIS_OPENCODE_MCP_TOKEN"))
                .unwrap()
                .unwrap(),
            std::ffi::OsStr::new(&mcp.token)
        );
        assert_eq!(
            env.get(std::ffi::OsStr::new("OPENCODE_CONFIG_PROJECT_DISABLE"))
                .unwrap()
                .unwrap(),
            "true"
        );
    }
}
