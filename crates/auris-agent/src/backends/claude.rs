//! Claude Code's bidirectional stream-json protocol; credentials stay with the CLI.

use crate::{Bridge, HistorySnapshot, host::ToolHost, live_mcp::LiveMcp, memory};
use futures::StreamExt;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    process::{Child, Command},
    sync::oneshot,
};
use tokio_util::codec::{FramedRead, LinesCodec};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
// The CLI may echo base64 images from a bounded 25 MiB reference turn.
const FRAME_LIMIT: usize = 40 * 1024 * 1024;
const NATIVE_TOOLS: &[&str] = &[
    "Bash",
    "Read",
    "Glob",
    "Grep",
    "Write",
    "Edit",
    "NotebookEdit",
];

fn native_operation(tool: &str) -> Option<&'static str> {
    match tool {
        "Bash" => Some("cli_command"),
        "Write" | "Edit" | "NotebookEdit" => Some("cli_file_change"),
        "Read" | "Glob" | "Grep" => Some("cli_file_read"),
        _ => None,
    }
}

fn tool_name(name: &str) -> Option<&str> {
    if native_operation(name).is_some() {
        return Some(name);
    }
    name.strip_prefix("mcp__auris_live__")
        .filter(|name| ToolHost::contains(name))
}

pub(crate) fn executable(prefs: &auris_session::AgentPreferences) -> Result<PathBuf, String> {
    if !prefs.claude_path.trim().is_empty() {
        let path = PathBuf::from(prefs.claude_path.trim());
        if cfg!(windows)
            && path
                .extension()
                .is_none_or(|ext| !ext.eq_ignore_ascii_case("exe"))
        {
            return Err(
                "Choose the native claude.exe executable, not a .cmd or .ps1 launcher".into(),
            );
        }
        return Ok(path);
    }
    let name = if cfg!(windows) {
        "claude.exe"
    } else {
        "claude"
    };
    let mut directories: Vec<_> =
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()).collect();
    if let Some(home) = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }) {
        directories.push(PathBuf::from(home).join(".local/bin"));
    }
    directories.into_iter().map(|dir| dir.join(name)).find(|path| path.is_file())
        .ok_or_else(|| "Claude Code CLI was not found. Install Claude Code, sign in with claude, or set its executable path in Agent settings".into())
}

fn command(
    prefs: &auris_session::AgentPreferences,
    cwd: &Path,
    native: bool,
) -> Result<Command, String> {
    let mut command = Command::new(executable(prefs)?);
    // A file, not an argument: the preamble outgrows Windows' 32 767-character command line.
    let prompt = cwd.join("system-prompt.md");
    std::fs::write(
        &prompt,
        if native {
            super::cli_preamble(prefs)
        } else {
            prefs.system_prompt.clone()
        },
    )
    .map_err(|error| format!("Cannot write the Claude Code system prompt: {error}"))?;
    command
        .args([
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--no-session-persistence",
            "--setting-sources=",
            "--strict-mcp-config",
            "--mcp-config",
        ])
        .arg(cwd.join("mcp.json"))
        .args([
            "--tools",
            if native { "Bash,Read,Glob,Grep,Write,Edit,NotebookEdit" } else { "" },
            "--disable-slash-commands",
            "--no-chrome",
            "--permission-mode",
            if native { "default" } else { "dontAsk" },
            "--permission-prompt-tool",
            "stdio",
            "--settings",
        ])
        .arg(
            json!({"disableAllHooks":true,"autoMemoryEnabled":false,"enabledPlugins":{},"fastMode":prefs.fast_mode,
            "permissions":{"allow":["mcp__auris_live__*"],"ask":if native { NATIVE_TOOLS } else { &[] }}})
            .to_string(),
        )
        .arg("--system-prompt-file")
        .arg(&prompt)
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    if !prefs.model.is_empty() {
        command.arg(format!("--model={}", prefs.model));
    }
    use auris_session::ReasoningEffort;
    let effort = match prefs.effort {
        ReasoningEffort::Default => None,
        ReasoningEffort::None | ReasoningEffort::Minimal | ReasoningEffort::Low => Some("low"),
        ReasoningEffort::Medium => Some("medium"),
        ReasoningEffort::High => Some("high"),
        ReasoningEffort::Xhigh => Some("xhigh"),
        ReasoningEffort::Max => Some("max"),
    };
    if let Some(effort) = effort {
        command.args(["--effort", effort]);
    }
    // Keep CLI authentication, but do not inherit a parent agent's runtime or tool pipes.
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if upper == "CLAUDECODE"
            || upper.starts_with("CODEX_") && upper != "CODEX_HOME"
            || upper.starts_with("CLAUDE_AGENT_SDK_")
        {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    Ok(command)
}

fn write_mcp_config(cwd: &Path, mcp: Option<&LiveMcp>) -> Result<(), String> {
    let servers = match mcp {
        Some(mcp) => json!({"auris_live":{"type":"http","url":mcp.url,
            "headers":{"Authorization":format!("Bearer {}", mcp.token)}}}),
        None => json!({}),
    };
    // The token lives only in the private temporary directory, never in process arguments.
    let mut file = tempfile::NamedTempFile::new_in(cwd).map_err(|error| error.to_string())?;
    std::io::Write::write_all(
        &mut file,
        json!({"mcpServers":servers}).to_string().as_bytes(),
    )
    .map_err(|error| error.to_string())?;
    file.persist_noclobber(cwd.join("mcp.json"))
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn working_directory() -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("auris-claude-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().map_err(|error| error.to_string())
}

struct Cli {
    child: Option<Child>,
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: FramedRead<Box<dyn AsyncRead + Unpin + Send>, LinesCodec>,
    next_id: u64,
    /// The folder the CLI works in, with its links resolved: the one place it may read
    /// without asking. `None` when there is no process behind this transport.
    root: Option<PathBuf>,
}

impl Cli {
    fn transport(
        child: Option<Child>,
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
    ) -> Self {
        Self {
            child,
            input: Box::new(input),
            output: FramedRead::new(
                Box::new(output),
                LinesCodec::new_with_max_length(FRAME_LIMIT),
            ),
            next_id: 0,
            root: None,
        }
    }

    async fn spawn(
        prefs: &auris_session::AgentPreferences,
        cwd: &Path,
        native: bool,
        project: Option<&Path>,
    ) -> Result<(Self, Value), String> {
        let root = project.unwrap_or(cwd);
        let mut child = command(prefs, cwd, native)?
            .current_dir(root)
            .spawn()
            .map_err(|error| format!("Cannot start Claude Code: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or("Claude Code stdin was not available")?;
        let output = child
            .stdout
            .take()
            .ok_or("Claude Code stdout was not available")?;
        let mut cli = Self::transport(Some(child), input, output);
        cli.root = std::fs::canonicalize(root).ok();
        let initialized = cli
            .request(json!({"subtype":"initialize","hooks":{},"agents":{},"skills":[]}))
            .await?;
        Ok((cli, initialized))
    }

    async fn send(&mut self, message: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|error| format!("Claude Code connection closed: {error}"))
    }

    async fn read(&mut self) -> Result<Value, String> {
        let line = self.output.next().await.ok_or("Claude Code exited or closed its connection. Check the CLI installation and sign in with claude")?
            .map_err(|error| error.to_string())?;
        serde_json::from_str(&line)
            .map_err(|error| format!("Invalid Claude Code protocol message: {error}"))
    }

    async fn reject_request(&mut self, message: &Value) -> Result<(), String> {
        if message["type"] == "control_request" {
            self.send(json!({"type":"control_response","response":{"subtype":"error",
                "request_id":message["request_id"],"error":"Unsupported or out-of-turn Claude Code request"}})).await?;
        }
        Ok(())
    }

    async fn request(&mut self, request: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = format!("auris-{}", self.next_id);
        self.send(json!({"type":"control_request","request_id":id,"request":request}))
            .await?;
        loop {
            let message = self.read().await?;
            self.reject_request(&message).await?;
            if message["type"] == "control_response" && message["response"]["request_id"] == id {
                let response = &message["response"];
                return if response["subtype"] == "success" {
                    Ok(response["response"].clone())
                } else {
                    Err(format!(
                        "Claude Code: {}",
                        response["error"]
                            .as_str()
                            .unwrap_or("control request failed")
                    ))
                };
            }
        }
    }

    async fn stop(&mut self) {
        // Revoke MCP first in the caller, then interrupt before terminating the child.
        let _ = tokio::time::timeout(
            Duration::from_millis(500),
            self.request(json!({"subtype":"interrupt"})),
        )
        .await;
        let _ = self.input.shutdown().await;
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill().await;
        }
    }
}

pub(crate) async fn list_models(prefs: &auris_session::AgentPreferences) -> Result<String, String> {
    let cwd = working_directory()?;
    write_mcp_config(cwd.path(), None)?;
    let (mut cli, initialized) =
        tokio::time::timeout(STARTUP_TIMEOUT, Cli::spawn(prefs, cwd.path(), false, None))
            .await
            .map_err(|_| "Claude Code startup timed out".to_string())??;
    cli.stop().await;
    let models = initialized["models"]
        .as_array()
        .ok_or("Claude Code did not return its supported models")?;
    let models: Vec<_> = models
        .iter()
        .filter_map(|model| model["value"].as_str().map(|name| json!({"name":name})))
        .collect();
    if models.is_empty() {
        return Err(
            "Claude Code returned no models. Sign in with claude before using this backend".into(),
        );
    }
    Ok(json!({"models":models}).to_string())
}

pub(crate) async fn complete(
    prefs: &auris_session::AgentPreferences,
    prompt: &str,
) -> Result<String, String> {
    let cwd = working_directory()?;
    write_mcp_config(cwd.path(), None)?;
    let (mut cli, _) = Cli::spawn(prefs, cwd.path(), false, None).await?;
    let result = async {
        cli.send(
            json!({"type":"user","session_id":"","parent_tool_use_id":null,
            "message":{"role":"user","content":prompt}}),
        )
        .await?;
        loop {
            let message = cli.read().await?;
            cli.reject_request(&message).await?;
            if message["type"] == "system"
                && message["subtype"] == "init"
                && message["tools"]
                    .as_array()
                    .is_none_or(|tools| !tools.is_empty())
            {
                return Err("Text-only agent exposed tools".into());
            }
            if message["type"] == "assistant"
                && message["message"]["content"]
                    .as_array()
                    .is_some_and(|parts| parts.iter().any(|part| part["type"] == "tool_use"))
            {
                return Err("Text-only agent attempted to use a tool".into());
            }
            if message["type"] == "result" {
                return if message["subtype"] == "success" && message["is_error"] != true {
                    message["result"]
                        .as_str()
                        .map(str::to_owned)
                        .ok_or("Missing text-only answer".into())
                } else {
                    Err("Text-only agent failed".into())
                };
            }
        }
    }
    .await;
    cli.stop().await;
    result
}

pub(crate) async fn run(
    prefs: &auris_session::AgentPreferences,
    bridge: &Bridge,
    history_path: Option<PathBuf>,
    fresh: bool,
    history: Option<HistorySnapshot>,
    cancelled: oneshot::Receiver<()>,
) -> Result<(), String> {
    let cwd = working_directory()?;
    let project = super::native_project_directory(history_path.as_deref());
    let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await?;
    write_mcp_config(cwd.path(), Some(&mcp))?;
    let mut cancelled = cancelled;
    let (mut cli, _) = tokio::select! {
        biased;
        _ = &mut cancelled => return Ok(()),
        result = tokio::time::timeout(STARTUP_TIMEOUT, Cli::spawn(prefs, cwd.path(), true, project)) =>
            result.map_err(|_| "Claude Code startup timed out".to_string())??,
    };
    let result = tokio::select! {
        biased;
        _ = &mut cancelled => Ok(()),
        _ = mcp.cancelled() => Err("Claude Code cancelled a pending live tool request. Inspect the document before continuing".into()),
        result = conversation(&mut cli, prefs, bridge, &mcp, history_path, fresh, history) => result,
    };
    mcp.abort_calls();
    cli.stop().await;
    result
}

async fn conversation(
    cli: &mut Cli,
    prefs: &auris_session::AgentPreferences,
    bridge: &Bridge,
    mcp: &LiveMcp,
    history_path: Option<PathBuf>,
    fresh: bool,
    history: Option<HistorySnapshot>,
) -> Result<(), String> {
    let preloaded = history.is_some();
    let (mut memory, mut generation) = match (history, history_path) {
        (Some(snapshot), _) => {
            let (memory, generation) = snapshot.into_parts();
            (memory, Some(generation))
        }
        (None, Some(path)) => {
            let (memory, generation) = bridge.open_history(&path, fresh)?;
            (memory, Some(generation))
        }
        _ => (memory::Memory::default(), None),
    };
    bridge.bind_production_history(generation.as_ref())?;
    if !preloaded {
        bridge.emit(json!({"event":"history","summary":memory.summary,"turns":memory.turns}));
    }
    let status = tokio::time::timeout(
        STARTUP_TIMEOUT,
        cli.request(json!({"subtype":"mcp_status"})),
    )
    .await
    .map_err(|_| "Claude Code MCP startup timed out".to_string())??;
    validate_servers(&status)?;
    bridge.emit(json!({"event":"ready","model":prefs.model}));
    let mut context = super::memory_context(&memory);
    while let Some(line) = bridge.receive().await {
        let wire: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if wire["compact"] == true {
            bridge.emit(json!({"event":"compacted","ok":false,"manual":true,"message":"Manual compaction is not available for the Claude Code backend yet"}));
            continue;
        }
        let (said, attachments) = crate::parse_say(&line)?;
        bridge.checkpoint_turn(
            generation.as_mut(),
            &memory,
            wire["display"].as_str().unwrap_or(&said),
        );
        let policy_context =
            serde_json::from_value::<auris_session::agent_policy::Policy>(wire["policy"].clone())
                .map(|policy| crate::toolbox::live_agent::permission_context(&policy))
                .unwrap_or_default();
        let prompt = format!("{context}\n{policy_context}\n{said}");
        let content = match crate::attachments::cli_input(prompt, attachments, bridge, true).await {
            Ok(content) => content,
            Err(message) => {
                bridge.emit(json!({"event":"error","message":message}));
                continue;
            }
        };
        context.clear();
        mcp.set_active(true);
        cli.send(
            json!({"type":"user","session_id":"","parent_tool_use_id":null,"client_composed":true,
            "message":{"role":"user","content":content}}),
        )
        .await?;
        let completed = complete_turn(cli, bridge, prefs.tool_failure_limit()).await;
        let answer = completed.answer;
        mcp.set_active(false);
        let display = wire["display"].as_str().unwrap_or(&said);
        match &answer {
            Ok(text) => memory.push(display, text),
            Err(error) => memory.push_interrupted(display, error),
        }
        if let Some(generation) = generation.as_mut()
            && let Err(error) = bridge.save_history(generation, &memory)
        {
            bridge.emit(json!({"event":"notice","message":format!("Conversation history was not saved: {error}")}));
        }
        match answer {
            Ok(text) => bridge.emit(json!({"event":"answer","text":text})),
            Err(message) if completed.terminal_result => {
                bridge.emit(json!({"event":"error","message":message}));
            }
            Err(message) => return Err(message),
        }
    }
    Ok(())
}

/// Whether a native `Read`, `Glob` or `Grep` stays inside `root`, the folder the CLI works in.
///
/// What it may read without asking. The path it names is resolved against `root` and through
/// every link; a glob may not climb out with `..` or start somewhere absolute. Anything that
/// cannot be shown to stay inside — a path that does not exist yet, an unknown tool — is not.
fn read_stays_inside(root: &Path, tool: &str, input: &Value) -> bool {
    let inside = |path: Option<&str>| {
        let path = path.map_or_else(|| root.to_path_buf(), |path| root.join(path));
        std::fs::canonicalize(path).is_ok_and(|path| path.starts_with(root))
    };
    let confined = |pattern: Option<&str>| {
        pattern.is_none_or(|pattern| {
            !pattern.starts_with(['/', '\\', '~'])
                && !Path::new(pattern).is_absolute()
                && !pattern.split(['/', '\\']).any(|part| part == "..")
        })
    };
    match tool {
        "Read" => input["file_path"]
            .as_str()
            .is_some_and(|path| inside(Some(path))),
        "Glob" => inside(input["path"].as_str()) && confined(input["pattern"].as_str()),
        "Grep" => inside(input["path"].as_str()) && confined(input["glob"].as_str()),
        _ => false,
    }
}

fn validate_servers(status: &Value) -> Result<(), String> {
    let servers = status["mcpServers"]
        .as_array()
        .ok_or("Claude Code did not return its MCP catalog")?;
    if servers.len() != 1
        || servers[0]["name"] != "auris_live"
        || servers[0]["status"] != "connected"
    {
        return Err(
            "Claude Code could not verify an exclusive connection to the Auris live MCP tools"
                .into(),
        );
    }
    Ok(())
}

fn validate_tools(message: &Value) -> Result<(), String> {
    let tools = message["tools"]
        .as_array()
        .ok_or("Claude Code did not return its tool catalog")?;
    if tools.is_empty()
        || tools
            .iter()
            .any(|tool| tool.as_str().is_none_or(|name| tool_name(name).is_none()))
    {
        return Err("Claude Code exposed tools outside the Auris live catalog".into());
    }
    Ok(())
}

/// A completed CLI result can leave its stream available for another user request.
struct CompletedTurn {
    answer: Result<String, String>,
    terminal_result: bool,
}

#[derive(Default)]
struct ToolRecovery {
    failures: crate::tool_failures::ToolFailures,
    last_failure: Option<(String, String)>,
    fresh_failure: bool,
    terminal_result: bool,
    continuations: usize,
}

impl ToolRecovery {
    fn observe(&mut self, tool: &str, args: &str, ok: bool, auris_tool: bool) {
        self.last_failure = if !ok && auris_tool {
            self.failures.record(tool, args, true);
            self.fresh_failure = true;
            Some((tool.to_owned(), args.to_owned()))
        } else {
            self.failures.record(tool, args, false);
            None
        };
    }
}

async fn complete_turn(cli: &mut Cli, bridge: &Bridge, limit: usize) -> CompletedTurn {
    let mut recovery = ToolRecovery::default();
    loop {
        recovery.fresh_failure = false;
        recovery.terminal_result = false;
        let answer = receive_turn_tracked(cli, bridge, &mut recovery).await;
        let finish = |answer| CompletedTurn {
            answer,
            terminal_result: recovery.terminal_result,
        };
        if answer.is_err() || !recovery.fresh_failure || !bridge.can_recover_tools() {
            return finish(answer);
        }
        let Some((tool, args)) = &recovery.last_failure else {
            return finish(answer);
        };
        if recovery.failures.exhausted(tool, args, limit)
            || recovery.continuations >= limit.saturating_sub(1)
        {
            return finish(Err(format!(
                "Claude Code tool recovery reached the configured limit of {limit}. Earlier edits remain in the document. Send a new instruction to continue."
            )));
        }
        recovery.continuations += 1;
        bridge.emit(json!({"event":"notice","message":"Claude Code ended immediately after a tool error; asking it to correct the call and continue the unfinished request."}));
        // Continue the existing CLI transcript. Never replay an edit or manufacture a result.
        let text = "Auris runtime recovery: the last Auris tool call failed. Continue only the unfinished original user request. Read tool_help and inspect the current document as needed, then correct the arguments or choose a valid alternative. Earlier successful edits remain applied: do not blindly replay them. Tool argument errors do not consume the musical polishing budget. Do not bypass any refusal or expand permissions. If user input or an unavailable resource is required, explain that and finish.";
        if let Err(error) = cli.send(json!({"type":"user","session_id":"","parent_tool_use_id":null,"client_composed":true,"message":{"role":"user","content":text}})).await {
            return CompletedTurn { answer: Err(error), terminal_result: false };
        }
    }
}

#[cfg(test)]
async fn receive_turn(cli: &mut Cli, bridge: &Bridge) -> Result<String, String> {
    receive_turn_tracked(cli, bridge, &mut ToolRecovery::default()).await
}

async fn receive_turn_tracked(
    cli: &mut Cli,
    bridge: &Bridge,
    recovery: &mut ToolRecovery,
) -> Result<String, String> {
    let mut answer = String::new();
    let mut streamed = false;
    let mut calls = std::collections::HashMap::new();
    loop {
        let message = cli.read().await?;
        if message["type"] == "control_request" {
            let request = &message["request"];
            if request["subtype"] == "can_use_tool"
                && request["input"].is_object()
                && let Some(operation) = request["tool_name"].as_str().and_then(native_operation)
            {
                let _guard = bridge.tool_lock().await;
                let within = cli.root.as_deref().is_some_and(|root| {
                    request["tool_name"]
                        .as_str()
                        .is_some_and(|tool| read_stays_inside(root, tool, &request["input"]))
                });
                let origin = crate::permissions::call_origin(
                    request["tool_use_id"].as_str(),
                    request["tool_name"]
                        .as_str()
                        .and_then(tool_name)
                        .unwrap_or(operation),
                    "claude_approval",
                );
                let result = crate::permissions::with_origin(origin, crate::permissions::authorize(Some(bridge), operation,
                    &json!({"backend":"claude","tool":request["tool_name"],"input":request["input"],
                        "within_project":within}).to_string())).await;
                let response = match result {
                    Ok(()) => json!({"behavior":"allow","updatedInput":request["input"]}),
                    Err(reason) => json!({"behavior":"deny","message":reason}),
                };
                cli.send(
                    json!({"type":"control_response","response":{"subtype":"success",
                    "request_id":message["request_id"],"response":response}}),
                )
                .await?;
            } else {
                cli.reject_request(&message).await?;
            }
            continue;
        }
        if !message["parent_tool_use_id"].is_null() {
            continue;
        }
        match message["type"].as_str().unwrap_or_default() {
            "system" if message["subtype"] == "init" => validate_tools(&message)?,
            "stream_event" => {
                let delta = &message["event"]["delta"];
                match delta["type"].as_str().unwrap_or_default() {
                    "text_delta" => {
                        let text = delta["text"].as_str().unwrap_or_default();
                        streamed = true;
                        bridge.emit(json!({"event":"text_delta","text":text}));
                    }
                    "thinking_delta" => {
                        bridge.emit(json!({"event":"reasoning_delta","text":delta["thinking"]}))
                    }
                    _ => {}
                }
            }
            "assistant" => {
                if let Some(content) = message["message"]["content"].as_array() {
                    for part in content {
                        match part["type"].as_str().unwrap_or_default() {
                            "text" => {
                                let text = part["text"].as_str().unwrap_or_default();
                                answer.push_str(text);
                                if !streamed {
                                    bridge.emit(json!({"event":"text_delta","text":text}));
                                }
                            }
                            "tool_use" => {
                                let tool = part["name"].as_str().and_then(tool_name).ok_or(
                                    "Claude Code requested a tool outside the Auris catalog",
                                )?;
                                let id = part["id"]
                                    .as_str()
                                    .ok_or("Claude Code omitted a tool call ID")?;
                                calls.insert(
                                    id.to_owned(),
                                    (
                                        tool.to_owned(),
                                        part["input"].to_string(),
                                        part["name"].as_str().is_some_and(|name| {
                                            name.starts_with("mcp__auris_live__")
                                        }),
                                    ),
                                );
                                bridge.emit(json!({"event":"phase","phase":"tool"}));
                                bridge.emit(json!({"event":"call","call_id":id,"tool":tool,"args":part["input"]}));
                            }
                            _ => {}
                        }
                    }
                }
                streamed = false;
            }
            "user" => {
                if let Some(content) = message["message"]["content"].as_array() {
                    for part in content.iter().filter(|part| part["type"] == "tool_result") {
                        let id = part["tool_use_id"].as_str().unwrap_or_default();
                        if let Some((tool, args, auris_tool)) = calls.remove(id) {
                            let text = match &part["content"] {
                                Value::String(text) => text.clone(),
                                Value::Array(parts) => parts
                                    .iter()
                                    .filter_map(|part| part["text"].as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            let ok = part["is_error"] != true;
                            recovery.observe(&tool, &args, ok, auris_tool);
                            bridge.emit(json!({"event":"result","call_id":id,"tool":tool,"ok":ok,"text":text}));
                        }
                    }
                }
            }
            "result" => {
                recovery.terminal_result = true;
                if message["is_error"] == true || message["subtype"] != "success" {
                    return Err(format!(
                        "Claude Code turn failed: {}",
                        message["errors"]
                            .as_array()
                            .map(|errors| errors
                                .iter()
                                .filter_map(Value::as_str)
                                .collect::<Vec<_>>()
                                .join("\n"))
                            .filter(|text| !text.is_empty())
                            .or_else(|| message["result"].as_str().map(str::to_owned))
                            .unwrap_or_else(|| message["subtype"]
                                .as_str()
                                .unwrap_or("unknown error")
                                .to_owned())
                    ));
                }
                if answer.is_empty() {
                    answer = message["result"].as_str().unwrap_or_default().to_owned();
                }
                return Ok(answer);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    async fn reply(writer: &mut (impl AsyncWrite + Unpin), value: Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    fn prefs() -> auris_session::AgentPreferences {
        auris_session::AgentPreferences {
            backend: "claude".into(),
            model: "sonnet".into(),
            ..Default::default()
        }
    }

    #[test]
    fn only_reads_that_stay_in_the_working_folder_are_free() {
        let folder = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(folder.path()).unwrap();
        std::fs::create_dir(root.join("Audio")).unwrap();
        std::fs::write(root.join("Song.auris"), b"{}").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();

        assert!(read_stays_inside(
            &root,
            "Read",
            &json!({"file_path":"Song.auris"})
        ));
        assert!(read_stays_inside(
            &root,
            "Read",
            &json!({"file_path": root.join("Song.auris")})
        ));
        assert!(!read_stays_inside(
            &root,
            "Read",
            &json!({"file_path": outside.path()})
        ));
        assert!(!read_stays_inside(
            &root,
            "Read",
            &json!({"file_path":"Audio/../../elsewhere"})
        ));
        assert!(!read_stays_inside(
            &root,
            "Read",
            &json!({"file_path":"missing.txt"})
        ));
        assert!(read_stays_inside(
            &root,
            "Glob",
            &json!({"pattern":"**/*.wav"})
        ));
        assert!(!read_stays_inside(
            &root,
            "Glob",
            &json!({"pattern":"../**/*.pem"})
        ));
        assert!(!read_stays_inside(
            &root,
            "Glob",
            &json!({"pattern":"~/.ssh/*"})
        ));
        assert!(read_stays_inside(
            &root,
            "Grep",
            &json!({"pattern":"tempo","path":"Audio"})
        ));
        assert!(!read_stays_inside(
            &root,
            "Grep",
            &json!({"pattern":"key","path": outside.path()})
        ));
        assert!(!read_stays_inside(
            &root,
            "Bash",
            &json!({"command":"cat Song.auris"})
        ));
    }

    #[test]
    fn catalogs_reject_unrelated_tools_and_disconnected_servers() {
        assert!(
            validate_servers(&json!({"mcpServers":[{"name":"auris_live","status":"connected"}]}))
                .is_ok()
        );
        for servers in [
            json!([]),
            json!([{"name":"auris_live","status":"failed"}]),
            json!([{"name":"auris_live","status":"connected"},{"name":"other","status":"connected"}]),
        ] {
            assert!(validate_servers(&json!({"mcpServers":servers})).is_err());
        }
        assert!(
            validate_tools(
                &json!({"tools":["mcp__auris_live__add_track", "mcp__auris_live__generate_phrase", "mcp__auris_live__inspect_performance", "mcp__auris_live__set_performance"]})
            )
            .is_ok()
        );
        for tools in [
            json!([]),
            json!(["Agent"]),
            json!(["mcp__other__add_track"]),
            json!(["mcp__auris_live__missing"]),
        ] {
            assert!(validate_tools(&json!({"tools":tools})).is_err());
        }
    }

    #[test]
    fn command_limits_tools_and_keeps_the_mcp_token_out_of_arguments() {
        let cwd = tempfile::tempdir().unwrap();
        let prefs = auris_session::AgentPreferences {
            system_prompt: "Custom music instructions\n二番も作る".into(),
            claude_path: if cfg!(windows) {
                "C:/Tools/claude.exe"
            } else {
                "/usr/local/bin/claude"
            }
            .into(),
            ..prefs()
        };
        let command = command(&prefs, cwd.path(), false).unwrap();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.windows(2).any(|args| args == ["--tools", ""]));
        assert!(args.iter().any(|arg| arg == "--strict-mcp-config"));
        assert!(args.iter().any(|arg| arg == "--setting-sources="));
        assert!(args.iter().any(|arg| arg == "--no-session-persistence"));
        assert!(
            !args
                .iter()
                .any(|arg| arg.contains("Bearer") || arg.contains("bypassPermissions"))
        );
        assert_eq!(command.as_std().get_current_dir(), Some(cwd.path()));
        let prompt = |args: &[String]| {
            let pair = args
                .windows(2)
                .find(|pair| pair[0] == "--system-prompt-file")
                .unwrap();
            std::fs::read_to_string(&pair[1]).unwrap()
        };
        assert_eq!(prompt(&args), prefs.system_prompt);
        assert!(!args.iter().any(|arg| arg == "--system-prompt"));
        let native = super::command(&prefs, cwd.path(), true).unwrap();
        let args: Vec<_> = native
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(prompt(&args), super::super::cli_preamble(&prefs));
        assert!(prompt(&args).starts_with(&prefs.system_prompt));
        // The whole command line stays far below Windows' 32 767-character limit.
        assert!(args.iter().map(|arg| arg.len() + 1).sum::<usize>() < 8 * 1024);
        assert!(
            args.windows(2)
                .any(|args| args == ["--permission-mode", "default"])
        );
        let settings = args
            .windows(2)
            .find(|args| args[0] == "--settings")
            .unwrap();
        let settings: Value = serde_json::from_str(&settings[1]).unwrap();
        assert_eq!(settings["permissions"]["ask"], json!(NATIVE_TOOLS));
        assert_eq!(settings["fastMode"], false);
        assert!(
            validate_tools(
                &json!({"tools":["Bash","Read","Edit","mcp__auris_live__inspect_project"]})
            )
            .is_ok()
        );
    }

    #[test]
    fn fast_mode_is_passed_in_session_settings_without_reducing_effort() {
        let cwd = tempfile::tempdir().unwrap();
        let prefs = auris_session::AgentPreferences {
            claude_path: if cfg!(windows) {
                "C:/Tools/claude.exe"
            } else {
                "/usr/local/bin/claude"
            }
            .into(),
            fast_mode: true,
            effort: auris_session::ReasoningEffort::High,
            ..prefs()
        };
        let command = command(&prefs, cwd.path(), true).unwrap();
        let args: Vec<_> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy())
            .collect();
        let settings = args
            .windows(2)
            .find(|pair| pair[0] == "--settings")
            .unwrap();
        let settings: Value = serde_json::from_str(&settings[1]).unwrap();
        assert_eq!(settings["fastMode"], true);
        assert!(args.windows(2).any(|pair| pair == ["--effort", "high"]));
    }

    #[tokio::test]
    async fn native_permissions_wait_for_the_desktop_and_preserve_exact_input() {
        for allowed in [true, false] {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (bridge, commands, events) = crate::worker::test_bridge();
                let (client, server) = tokio::io::duplex(32768);
                let (reader, writer) = tokio::io::split(client);
                let mut cli = Cli::transport(None, writer, reader);
                let input = json!({"command":"python export.py","description":"Export the requested WAV"});
                let peer = async {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();
                    reply(&mut writer, json!({"type":"control_request","request_id":"native-1","request":{"subtype":"can_use_tool","tool_name":"Bash","input":input}})).await;
                    let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(response["response"]["request_id"], "native-1");
                    assert_eq!(response["response"]["response"]["behavior"], if allowed { "allow" } else { "deny" });
                    if allowed { assert_eq!(response["response"]["response"]["updatedInput"], input); }
                    reply(&mut writer, json!({"type":"result","subtype":"success","result":"Finished"})).await;
                };
                let owner = async {
                    let event = loop {
                        match events.try_recv() { Ok(event) => break event, Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await, Err(error) => panic!("{error}") }
                    };
                    assert_eq!(event["tool"], "cli_command");
                    assert_eq!(event["args"]["input"], input);
                    commands.send(json!({"event":"permission_result","id":event["id"],"ok":allowed,"reason":"fixture"}).to_string()).unwrap();
                };
                let (answer, (), ()) = tokio::join!(receive_turn(&mut cli, &bridge), peer, owner);
                assert_eq!(answer.unwrap(), "Finished");
            }).await.unwrap();
        }
    }

    #[tokio::test]
    async fn mcp_configuration_keeps_credentials_in_a_private_temporary_file() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
        let cwd = working_directory().unwrap();
        write_mcp_config(cwd.path(), Some(&mcp)).unwrap();
        let path = cwd.path().join("mcp.json");
        let config: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            config["mcpServers"]["auris_live"]["headers"]["Authorization"],
            format!("Bearer {}", mcp.token)
        );
        assert_eq!(config["mcpServers"].as_object().unwrap().len(), 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o077,
                0
            );
            assert_eq!(
                std::fs::metadata(cwd.path()).unwrap().permissions().mode() & 0o077,
                0
            );
        }
        drop(cwd);
        assert!(!path.exists());
    }

    #[tokio::test]
    async fn streaming_text_thinking_and_tool_results_are_normalized_without_duplicates() {
        let (bridge, _commands, events) = crate::worker::test_bridge();
        let (client, server) = tokio::io::duplex(32768);
        let (reader, writer) = tokio::io::split(client);
        let mut cli = Cli::transport(None, writer, reader);
        let drive = async { receive_turn(&mut cli, &bridge).await.unwrap() };
        let peer = async {
            let (_, mut writer) = tokio::io::split(server);
            reply(
                &mut writer,
                json!({"type":"system","subtype":"init","tools":["mcp__auris_live__add_track"]}),
            )
            .await;
            reply(&mut writer, json!({"type":"stream_event","event":{"delta":{"type":"thinking_delta","thinking":"Planning"}}})).await;
            reply(&mut writer, json!({"type":"stream_event","event":{"delta":{"type":"text_delta","text":"Added."}}})).await;
            reply(&mut writer, json!({"type":"assistant","message":{"content":[{"type":"text","text":"Added."},{"type":"tool_use","id":"call","name":"mcp__auris_live__add_track","input":{"name":"Lead"}}]}})).await;
            reply(&mut writer, json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"call","content":[{"type":"text","text":"Done"},{"type":"image","data":"do not store"}]}]}})).await;
            reply(&mut writer, json!({"type":"assistant","parent_tool_use_id":"other","message":{"content":[{"type":"text","text":"Ignore"}]}})).await;
            reply(
                &mut writer,
                json!({"type":"result","subtype":"success","is_error":false,"result":"Added."}),
            )
            .await;
        };
        let (answer, ()) = tokio::join!(drive, peer);
        assert_eq!(answer, "Added.");
        let events: Vec<_> = events.try_iter().collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event"] == "text_delta")
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "reasoning_delta" && event["text"] == "Planning")
        );
        assert!(events.iter().any(|event| event["event"] == "result"
            && event["tool"] == "add_track"
            && event["text"] == "Done"
            && event["ok"] == true));
    }

    async fn emit_tool_result(
        writer: &mut (impl AsyncWrite + Unpin),
        id: &str,
        name: &str,
        args: Value,
        ok: bool,
    ) {
        reply(writer, json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":id,"name":name,"input":args}]}})).await;
        reply(writer, json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":!ok,"content":if ok {"Done"} else {"Invalid arguments"}}]}})).await;
    }

    #[tokio::test]
    async fn tool_failures_resume_claude_within_the_budget_but_never_after_a_denial() {
        tokio::time::timeout(Duration::from_secs(10), async {
            for (limit, denied, recover) in [(5, false, true), (2, false, false), (5, true, false)] {
                let (bridge, _commands, events) = crate::worker::test_bridge();
                if denied { bridge.block_tool_recovery(); }
                let (client, server) = tokio::io::duplex(65536);
                let (reader, writer) = tokio::io::split(client);
                let mut cli = Cli::transport(None, writer, reader);
                let peer = async {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();
                    for id in ["bad-1", "bad-2"] {
                        emit_tool_result(&mut writer, id, "mcp__auris_live__add_track", json!({"kind":17}), false).await;
                    }
                    reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":"Stopped after errors"})).await;
                    if recover {
                        let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                        let text = request["message"]["content"].as_str().unwrap();
                        assert!(text.contains("do not blindly replay"));
                        assert!(text.contains("Do not bypass any refusal"));
                        emit_tool_result(&mut writer, "fixed", "mcp__auris_live__add_track", json!({"name":"Lead","kind":"instrument"}), true).await;
                        reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":"Recovered"})).await;
                    }
                };
                let (completed, ()) = tokio::join!(complete_turn(&mut cli, &bridge, limit), peer);
                assert!(completed.terminal_result);
                if recover { assert_eq!(completed.answer.unwrap(), "Recovered"); }
                else if denied { assert_eq!(completed.answer.unwrap(), "Stopped after errors"); }
                else { assert!(completed.answer.unwrap_err().contains("limit of 2")); }
                assert_eq!(events.try_iter().filter(|event| event["event"] == "notice").count(), usize::from(recover));
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn a_reply_without_new_failed_tools_and_a_native_command_failure_are_not_repeated() {
        tokio::time::timeout(Duration::from_secs(10), async {
        for native in [false, true] {
            let (bridge, _commands, events) = crate::worker::test_bridge();
            let (client, server) = tokio::io::duplex(65536);
            let (reader, writer) = tokio::io::split(client);
            let mut cli = Cli::transport(None, writer, reader);
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                emit_tool_result(
                    &mut writer,
                    "bad",
                    if native {
                        "Bash"
                    } else {
                        "mcp__auris_live__add_track"
                    },
                    json!({}),
                    false,
                )
                .await;
                reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":"Need user input"})).await;
                if !native {
                    lines.next_line().await.unwrap().unwrap();
                    reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":"Need user input"})).await;
                }
            };
            let (completed, ()) = tokio::join!(complete_turn(&mut cli, &bridge, 5), peer);
            assert_eq!(completed.answer.unwrap(), "Need user input");
            assert_eq!(
                events
                    .try_iter()
                    .filter(|event| event["event"] == "notice")
                    .count(),
                usize::from(!native)
            );
        }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn changing_failed_arguments_cannot_extend_automatic_recovery_forever() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (bridge, _commands, events) = crate::worker::test_bridge();
            let (client, server) = tokio::io::duplex(65536);
            let (reader, writer) = tokio::io::split(client);
            let mut cli = Cli::transport(None, writer, reader);
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                for attempt in 0..3 {
                    if attempt > 0 {
                        lines.next_line().await.unwrap().unwrap();
                    }
                    emit_tool_result(
                        &mut writer,
                        &format!("bad-{attempt}"),
                        "mcp__auris_live__add_track",
                        json!({"kind":attempt}),
                        false,
                    )
                    .await;
                    reply(
                        &mut writer,
                        json!({"type":"result","subtype":"success",
                        "is_error":false,"result":"Stopped after an error"}),
                    )
                    .await;
                }
            };
            let (completed, ()) = tokio::join!(complete_turn(&mut cli, &bridge, 3), peer);
            assert!(completed.terminal_result);
            assert!(completed.answer.unwrap_err().contains("limit of 3"));
            assert_eq!(
                events
                    .try_iter()
                    .filter(|event| event["event"] == "notice")
                    .count(),
                2
            );
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn a_cli_turn_error_keeps_the_conversation_available_for_the_next_request() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
            let (client, server) = tokio::io::duplex(65536);
            let (reader, writer) = tokio::io::split(client);
            let mut cli = Cli::transport(None, writer, reader);
            commands.send(json!({"say":"First request"}).to_string()).unwrap();
            commands.send(json!({"say":"Second request"}).to_string()).unwrap();
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                reply(&mut writer, json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{"mcpServers":[{"name":"auris_live","status":"connected"}]}}})).await;
                lines.next_line().await.unwrap().unwrap();
                reply(&mut writer, json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["recoverable fixture turn error"]})).await;
                lines.next_line().await.unwrap().unwrap();
                reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":"Second request finished"})).await;
                drop(commands);
            };
            let preferences = prefs();
            let (result, ()) = tokio::join!(conversation(&mut cli, &preferences, &bridge, &mcp, None, false, None), peer);
            result.unwrap();
            let events: Vec<_> = events.try_iter().collect();
            assert!(events.iter().any(|event| event["event"] == "error" && event["message"].as_str().is_some_and(|text| text.contains("fixture turn error"))));
            assert!(events.iter().any(|event| event["event"] == "answer" && event["text"] == "Second request finished"));
        }).await.unwrap();
    }

    #[tokio::test]
    async fn control_requests_are_refused_and_turn_errors_are_reported() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let (client, server) = tokio::io::duplex(32768);
        let (reader, writer) = tokio::io::split(client);
        let mut cli = Cli::transport(None, writer, reader);
        let peer = async {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            reply(&mut writer, json!({"type":"control_request","request_id":"unexpected","request":{"subtype":"can_use_tool","tool_name":"Bash"}})).await;
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["response"]["request_id"], "unexpected");
            assert_eq!(response["response"]["subtype"], "error");
            reply(&mut writer, json!({"type":"result","subtype":"error_during_execution","is_error":true,"errors":["Please sign in"]})).await;
        };
        let (result, ()) = tokio::join!(receive_turn(&mut cli, &bridge), peer);
        assert!(result.unwrap_err().contains("Please sign in"));
    }

    #[tokio::test]
    async fn stop_interrupts_the_cli_before_closing_its_input() {
        let (client, server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut cli = Cli::transport(None, writer, reader);
        let peer = async {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["request"]["subtype"], "interrupt");
            reply(&mut writer, json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{}}})).await;
            assert!(lines.next_line().await.unwrap().is_none());
        };
        tokio::join!(cli.stop(), peer);
    }

    #[tokio::test]
    async fn restored_history_is_sent_once_and_completed_turns_are_saved() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("history.json");
            let mut memory = memory::Memory::default();
            memory.push("Remember the old request", "Old response");
            memory.save(&path).unwrap();
            let (bridge, commands, _events) = crate::worker::test_bridge();
            let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
            let (client, server) = tokio::io::duplex(32768);
            let (reader, writer) = tokio::io::split(client);
            let mut cli = Cli::transport(None, writer, reader);
            commands.send(json!({"say":"First request","display":"First display"}).to_string()).unwrap();
            commands.send(json!({"say":"Second request"}).to_string()).unwrap();
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                reply(&mut writer, json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{"mcpServers":[{"name":"auris_live","status":"connected"}]}}})).await;
                for index in 0..2 {
                    let prompt: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    let text = prompt["message"]["content"].as_str().unwrap();
                    assert_eq!(text.contains("Remember the old request"), index == 0);
                    let checkpoint = memory::Memory::load(&path).unwrap();
                    assert!(checkpoint.turns.last().unwrap().answer.contains("no final answer was recorded"));
                    reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":format!("Response {index}")})).await;
                }
                drop(commands);
            };
            let preferences = prefs();
            let (result, ()) = tokio::join!(conversation(&mut cli, &preferences, &bridge, &mcp, Some(path.clone()), false, None), peer);
            result.unwrap();
            let memory = memory::Memory::load(&path).unwrap();
            assert_eq!(memory.turns.len(), 3);
            assert_eq!(memory.turns[1].user, "First display");
            assert_eq!(memory.turns[2].answer, "Response 1");
        }).await.unwrap();
    }

    /// A scripted Claude peer uses the real MCP endpoint, permission gate and document history.
    #[tokio::test]
    async fn claude_mcp_imports_external_notes_and_respects_denied_permissions() {
        for allowed in [true, false] {
            tokio::time::timeout(Duration::from_secs(10), async {
                let (bridge, commands, events) = crate::worker::test_bridge();
                let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
                let (client, server) = tokio::io::duplex(32768);
                let (reader, writer) = tokio::io::split(client);
                let mut cli = Cli::transport(None, writer, reader);
                let references = tempfile::tempdir().unwrap();
                let audio = references.path().join("reference.wav");
                crate::attachments::tests::reference_wav(&audio, false);
                commands.send(json!({"say":"Add a track","attachments":[audio]}).to_string()).unwrap();
                let preferences = prefs();
                let drive = async { conversation(&mut cli, &preferences, &bridge, &mcp, None, false, None).await.unwrap() };
                let peer = async {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();
                    let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(request["request"]["subtype"], "mcp_status");
                    reply(&mut writer, json!({"type":"control_response","response":{"subtype":"success","request_id":request["request_id"],"response":{"mcpServers":[{"name":"auris_live","status":"connected"}]}}})).await;
                    let prompt: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(prompt["type"], "user");
                    assert_eq!(prompt["client_composed"], true);
                    assert!(prompt["message"]["content"][1]["text"].as_str().unwrap().contains("local CPU audio measurements"));
                    let client = reqwest::Client::new();
                    let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token).header("Accept", "application/json, text/event-stream").json(&value);
                    let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"claude-fixture","version":"1"}}})).send().await.unwrap();
                    let session = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
                    request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session).send().await.unwrap();
                    let result=crate::live_mcp::transcription_test_call(&client,&mcp.url,&mcp.token,&session,"Claude lead").await;
                    assert_eq!(result["result"]["isError"] == true, !allowed, "{result}");
                    reply(&mut writer, json!({"type":"result","subtype":"success","is_error":false,"result":"Finished"})).await;
                };
                let owner = async {
                    let mut session = auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
                    let before = session.project().clone();
                    let mut edits = 0;
                    loop {
                        let event = match events.try_recv() {
                            Ok(event) => event,
                            Err(std::sync::mpsc::TryRecvError::Empty) => { tokio::task::yield_now().await; continue; }
                            Err(error) => panic!("{error}"),
                        };
                        match event["event"].as_str().unwrap_or_default() {
                            "permission" => {
                                commands.send(json!({"event":"permission_result","id":event["id"],"ok":allowed,"reason":"fixture policy"}).to_string()).unwrap();
                            }
                            "edit" => {
                                assert!(allowed);
                                edits += 1;
                                let text = session.agent_command(serde_json::from_value(event["command"].clone()).unwrap()).unwrap();
                                commands.send(json!({"event":"edit_result","ok":true,"text":text}).to_string()).unwrap();
                            }
                            "answer" => break,
                            _ => {}
                        }
                    }
                    assert_eq!(edits, usize::from(allowed));
                    if allowed { assert!(session.undo().is_some()); }
                    assert_eq!(*session.project(), before);
                    drop(commands);
                };
                tokio::join!(drive, peer, owner);
            }).await.unwrap();
        }
    }

    #[tokio::test]
    #[ignore = "requires an installed Claude Code CLI; does not run inference"]
    async fn installed_claude_connects_to_the_live_mcp_catalog() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let cwd = tempfile::tempdir().unwrap();
        let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
        write_mcp_config(cwd.path(), Some(&mcp)).unwrap();
        let (mut cli, initialized) = tokio::time::timeout(
            STARTUP_TIMEOUT,
            Cli::spawn(&prefs(), cwd.path(), true, None),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(!initialized["models"].as_array().unwrap().is_empty());
        let status = tokio::time::timeout(
            STARTUP_TIMEOUT,
            cli.request(json!({"subtype":"mcp_status"})),
        )
        .await
        .unwrap()
        .unwrap();
        cli.stop().await;
        validate_servers(&status).unwrap_or_else(|error| panic!("{error}: {status}"));
    }

    #[test]
    #[ignore = "requires Claude Code sign-in and runs inference against an in-memory song"]
    fn installed_claude_edits_an_unsaved_song() {
        let mut preferences = prefs();
        preferences.model = "haiku".into();
        let mut worker = crate::Worker::spawn(preferences, None, true, None).unwrap();
        worker.send(&json!({"say":"Transport smoke test: call add_track exactly once with name Claude smoke lead and kind instrument. Do not change anything else, search for instruments, or render audio. No notes are needed. Report completion.","policy":auris_session::agent_policy::Policy::default()}).to_string()).unwrap();
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "Claude Code smoke test timed out"
            );
            let event = match worker.try_recv() {
                Ok(event) => event,
                Err(std::sync::mpsc::TryRecvError::Empty) => {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => panic!("{error}"),
            };
            match event["event"].as_str().unwrap_or_default() {
                "permission" => {
                    let operation = auris_session::agent_policy::Operation::parse(
                        event["tool"].as_str().unwrap(),
                        &event["args"],
                    )
                    .unwrap();
                    let allowed = operation.name == "edit_project.add_track";
                    worker
                        .send(
                            &json!({"event":"permission_result","id":event["id"],"ok":allowed})
                                .to_string(),
                        )
                        .unwrap();
                }
                "edit" => {
                    assert_eq!(event["command"]["action"], "import_transcription");
                    assert!(
                        event["command"]["transcription"]["notes"]
                            .as_array()
                            .unwrap()
                            .len()
                            >= 3
                    );
                    let result = session
                        .agent_command(serde_json::from_value(event["command"].clone()).unwrap());
                    worker.send(&json!({"event":"edit_result","ok":result.is_ok(),"text":result.unwrap_or_else(|error| error)}).to_string()).unwrap();
                }
                "error" => panic!("{}", event["message"]),
                "ended" => panic!("Claude Code ended before answering"),
                "answer" => break,
                _ => {}
            }
        }
        assert!(
            session
                .project()
                .tracks
                .iter()
                .any(|track| track.name == "Claude smoke lead")
        );
        assert!(session.path().is_none());
        assert!(session.is_dirty());
        assert!(session.undo().is_some());
        assert_eq!(session.project(), &before);
        worker.cancel();
    }
}
