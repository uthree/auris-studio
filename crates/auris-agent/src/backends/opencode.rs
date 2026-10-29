//! OpenCode's ACP v1 stdio transport; provider authentication stays with the CLI.

use crate::{Bridge, HistorySnapshot, host::ToolHost, live_mcp::LiveMcp, memory};
use futures::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
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
const FRAME_LIMIT: usize = 40 * 1024 * 1024;
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

fn cli_major(version: &str) -> Result<u32, String> {
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
    format!(
        "{}\nOpenCode exposes the live tools as auris_live_<tool name>, for example auris_live_inspect_project. Call them directly. Do not use native shell commands or directory scans to discover Auris tools. If the live tools are unavailable, report the connection problem instead.",
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
            "agents":{"auris":{"mode":"primary", "system":if native { opencode_preamble(prefs) } else { prefs.system_prompt.clone() },
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
        "agent":{"auris":{"mode":"primary", "prompt":if native { opencode_preamble(prefs) } else { prefs.system_prompt.clone() },
            "permission":permission, "tools":tools}}})
}

fn command(
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

struct Cli {
    child: Option<Child>,
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: FramedRead<Box<dyn AsyncRead + Unpin + Send>, LinesCodec>,
    next_id: u64,
    session: String,
    image: bool,
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
            session: String::new(),
            image: false,
        }
    }

    async fn spawn(
        prefs: &auris_session::AgentPreferences,
        cwd: &Path,
        project: Option<&Path>,
        mcp: Option<&LiveMcp>,
        native: bool,
    ) -> Result<(Self, Value), String> {
        let mut version_command = Command::new(executable(prefs)?);
        #[cfg(windows)]
        version_command.creation_flags(0x08000000);
        let version = tokio::time::timeout(
            Duration::from_secs(5),
            version_command.arg("--version").kill_on_drop(true).output(),
        )
        .await
        .map_err(|_| "OpenCode version check timed out".to_string())?
        .map_err(|error| format!("Cannot check OpenCode version: {error}"))?;
        let major = cli_major(&String::from_utf8_lossy(&version.stdout))?;
        let mut child = command(prefs, cwd, native, major, mcp)?
            .current_dir(project.unwrap_or(cwd))
            .spawn()
            .map_err(|error| format!("Cannot start OpenCode: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or("OpenCode stdin was not available")?;
        let output = child
            .stdout
            .take()
            .ok_or("OpenCode stdout was not available")?;
        let mut cli = Self::transport(Some(child), input, output);
        let session = cli
            .initialize(project.unwrap_or(cwd), if major == 1 { mcp } else { None })
            .await?;
        if let Some(mcp) = mcp {
            tokio::time::timeout(STARTUP_TIMEOUT, mcp.wait_for_catalog())
                .await
                .map_err(|_| {
                    "OpenCode did not connect to the Auris live tool catalogue".to_string()
                })?;
        }
        Ok((cli, session))
    }

    async fn initialize(&mut self, root: &Path, mcp: Option<&LiveMcp>) -> Result<Value, String> {
        let initialized = self.request("initialize", json!({"protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "clientInfo":{"name":"auris-studio","version":env!("CARGO_PKG_VERSION")}}), None).await?;
        if initialized["protocolVersion"] != 1 {
            return Err("OpenCode requires a compatible ACP v1 CLI".into());
        }
        if mcp.is_some() && initialized["agentCapabilities"]["mcpCapabilities"]["http"] != true {
            return Err(
                "OpenCode does not support the HTTP MCP transport required by Auris".into(),
            );
        }
        self.image = initialized["agentCapabilities"]["promptCapabilities"]["image"] == true;
        let servers = mcp.map_or_else(
            || json!([]),
            |mcp| {
                json!([{"type":"http","name":"auris_live","url":mcp.url,
            "headers":[{"name":"Authorization","value":format!("Bearer {}", mcp.token)}]}])
            },
        );
        let root = std::fs::canonicalize(root).map_err(|error| error.to_string())?;
        let session = self
            .request(
                "session/new",
                json!({"cwd":root,"mcpServers":servers}),
                None,
            )
            .await?;
        self.session = session["sessionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("OpenCode did not return an ACP session ID")?
            .to_owned();
        // Use the configured primary agent, independently of the CLI's default mode.
        if session["modes"]["availableModes"]
            .as_array()
            .is_some_and(|modes| modes.iter().any(|mode| mode["id"] == "auris"))
        {
            self.request(
                "session/set_mode",
                json!({"sessionId":self.session,"modeId":"auris"}),
                None,
            )
            .await?;
        }
        Ok(session)
    }

    async fn send(&mut self, message: Value) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(&message).map_err(|error| error.to_string())?;
        bytes.push(b'\n');
        self.input
            .write_all(&bytes)
            .await
            .map_err(|error| format!("OpenCode connection closed: {error}"))
    }

    async fn read(&mut self) -> Result<Value, String> {
        let line = self.output.next().await.ok_or("OpenCode exited or closed its connection. Check its installation and run opencode auth login")?.map_err(|error| error.to_string())?;
        let message: Value = serde_json::from_str(&line)
            .map_err(|error| format!("Invalid OpenCode ACP message: {error}"))?;
        if message["jsonrpc"] != "2.0" {
            return Err("Invalid OpenCode JSON-RPC version".into());
        }
        Ok(message)
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        bridge: Option<&Bridge>,
    ) -> Result<Value, String> {
        self.next_id += 1;
        let id = format!("auris-{}", self.next_id);
        self.send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}))
            .await?;
        let mut turn = Turn::default();
        loop {
            let message = self.read().await?;
            if message["method"].is_string() {
                if message.get("id").is_some() {
                    self.server_request(&message, bridge, &turn).await?;
                } else if message["method"] == "session/update"
                    && message["params"]["sessionId"] == self.session
                {
                    turn.update(&message["params"]["update"], bridge)?;
                }
                continue;
            }
            if message["id"] != id {
                continue;
            }
            if let Some(error) = message.get("error") {
                return Err(format!(
                    "OpenCode: {}",
                    error["message"].as_str().unwrap_or("ACP request failed")
                ));
            }
            let mut result = message
                .get("result")
                .cloned()
                .ok_or("OpenCode omitted the ACP result")?;
            if method == "session/prompt" {
                if result["stopReason"] != "end_turn" {
                    return Err(format!(
                        "OpenCode turn did not complete: {}",
                        result["stopReason"].as_str().unwrap_or("unknown reason")
                    ));
                }
                result["answer"] = json!(turn.answer);
            }
            return Ok(result);
        }
    }

    async fn server_request(
        &mut self,
        message: &Value,
        bridge: Option<&Bridge>,
        turn: &Turn,
    ) -> Result<(), String> {
        let params = &message["params"];
        if message["method"] != "session/request_permission" {
            return self.send(json!({"jsonrpc":"2.0","id":message["id"],"error":{"code":-32601,"message":"Unsupported OpenCode client request"}})).await;
        }
        let tool = &params["toolCall"];
        let operation = match tool["kind"].as_str().unwrap_or_default() {
            "execute" => Some("cli_command"),
            "edit" | "delete" | "move" => Some("cli_file_change"),
            "read" | "search" => Some("cli_file_read"),
            // This grants directory access once; the actual command or edit still
            // requires its own permission decision. Never make the grant persistent.
            _ if tool["title"] == "external_directory" => Some("cli_file_read"),
            _ => None,
        };
        let mut allowed = false;
        if params["sessionId"] == self.session
            && let (Some(bridge), Some(operation)) = (bridge, operation)
        {
            let _guard = bridge.tool_lock().await;
            let call_id = tool["toolCallId"].as_str();
            let original_tool = call_id
                .and_then(|id| turn.calls.get(id))
                .map(|(name, _, _)| name.as_str())
                .or_else(|| {
                    tool["name"]
                        .as_str()
                        .filter(|name| NATIVE_TOOLS.contains(name))
                })
                .or_else(|| {
                    tool["title"]
                        .as_str()
                        .filter(|name| NATIVE_TOOLS.contains(name))
                });
            let origin = match original_tool {
                Some(name) => crate::permissions::call_origin(call_id, name, "opencode_approval"),
                None => crate::permissions::PermissionOrigin::Independent {
                    source: "opencode_approval_without_tool_identity".into(),
                },
            };
            allowed = crate::permissions::with_origin(origin, crate::permissions::authorize(Some(bridge), operation,
                &json!({"backend":"opencode","tool":tool["title"],"input":tool["rawInput"],"request":tool,"within_project":false}).to_string())).await.is_ok();
        }
        let kind = if allowed { "allow_once" } else { "reject_once" };
        let option = params["options"].as_array().and_then(|options| {
            options
                .iter()
                .find(|option| option["kind"] == kind && option["optionId"].is_string())
        });
        let outcome = option.map_or_else(
            || json!({"outcome":"cancelled"}),
            |option| json!({"outcome":"selected","optionId":option["optionId"]}),
        );
        self.send(json!({"jsonrpc":"2.0","id":message["id"],"result":{"outcome":outcome}}))
            .await
    }

    async fn select_model(
        &mut self,
        prefs: &auris_session::AgentPreferences,
        session: &Value,
    ) -> Result<(), String> {
        if prefs.model.is_empty() {
            return Ok(());
        }
        if session["models"]["availableModels"].is_array() {
            self.request(
                "session/set_model",
                json!({"sessionId":self.session,"modelId":prefs.model}),
                None,
            )
            .await?;
        } else {
            let config =
                model_config(session).ok_or("OpenCode did not advertise model selection")?;
            self.request(
                "session/set_config_option",
                json!({"sessionId":self.session,"configId":config["id"],"value":prefs.model}),
                None,
            )
            .await?;
        }
        Ok(())
    }

    async fn prompt(&mut self, content: Value, bridge: Option<&Bridge>) -> Result<String, String> {
        let answer = self
            .request(
                "session/prompt",
                json!({"sessionId":self.session,"prompt":content}),
                bridge,
            )
            .await?;
        Ok(answer["answer"].as_str().unwrap_or_default().to_owned())
    }

    async fn stop(&mut self) {
        if !self.session.is_empty() {
            let _ = tokio::time::timeout(Duration::from_millis(500), self.send(json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.session}}))).await;
        }
        let _ = self.input.shutdown().await;
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill().await;
        }
    }
}

#[derive(Default)]
struct Turn {
    answer: String,
    calls: HashMap<String, (String, bool, Value)>,
}

impl Turn {
    fn update(&mut self, update: &Value, bridge: Option<&Bridge>) -> Result<(), String> {
        match update["sessionUpdate"].as_str().unwrap_or_default() {
            "agent_message_chunk" | "agent_thought_chunk"
                if update["content"]["type"] == "text" =>
            {
                let text = update["content"]["text"].as_str().unwrap_or_default();
                let thought = update["sessionUpdate"] == "agent_thought_chunk";
                if !thought {
                    self.answer.push_str(text);
                }
                if let Some(bridge) = bridge {
                    bridge.emit(json!({"event":if thought { "reasoning_delta" } else { "text_delta" },"text":text}));
                }
            }
            "tool_call" | "tool_call_update" => {
                let bridge = bridge.ok_or("Text-only agent attempted to use a tool")?;
                let id = update["toolCallId"]
                    .as_str()
                    .ok_or("OpenCode omitted a tool call ID")?;
                if update["sessionUpdate"] == "tool_call" {
                    let name = update["name"]
                        .as_str()
                        .or_else(|| update["title"].as_str())
                        .ok_or("OpenCode omitted a tool name")?;
                    let name = name
                        .strip_prefix("auris_live_")
                        .filter(|name| ToolHost::contains(name))
                        .or_else(|| NATIVE_TOOLS.contains(&name).then_some(name))
                        .ok_or_else(|| {
                            format!("OpenCode requested a tool outside the Auris catalog: {name}")
                        })?;
                    self.calls
                        .insert(id.to_owned(), (name.to_owned(), false, json!({})));
                }
                if let Some((name, emitted, args)) = self.calls.get_mut(id) {
                    if let Some(input) = update["rawInput"].as_object() {
                        for (key, value) in input {
                            args[key] = value.clone();
                        }
                    }
                    let done = matches!(update["status"].as_str(), Some("completed" | "failed"));
                    if !*emitted
                        && (done
                            || update["status"] == "in_progress"
                                && args.as_object().is_some_and(|input| !input.is_empty()))
                    {
                        bridge.emit(json!({"event":"phase","phase":"tool"}));
                        bridge.emit(json!({"event":"call","call_id":id,"tool":name,"args":args}));
                        *emitted = true;
                    }
                    if done {
                        let text = update["content"]
                            .as_array()
                            .map(|parts| {
                                parts
                                    .iter()
                                    .filter_map(|part| part["content"]["text"].as_str())
                                    .collect::<Vec<_>>()
                                    .join("\n")
                            })
                            .unwrap_or_default();
                        bridge.emit(json!({"event":"result","call_id":id,"tool":name,"ok":update["status"] == "completed","text":text}));
                        self.calls.remove(id);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn model_config(session: &Value) -> Option<&Value> {
    session["configOptions"]
        .as_array()?
        .iter()
        .find(|config| config["category"] == "model" || config["id"] == "model")
}

fn models(session: &Value) -> Result<String, String> {
    let mut result = Vec::new();
    if let Some(entries) = session["models"]["availableModels"].as_array() {
        for entry in entries {
            if let Some(id) = entry["modelId"].as_str().filter(|id| !id.is_empty()) {
                result.push(json!({"name":id,"display_name":entry["name"].as_str().unwrap_or(id),"efforts":[]}));
            }
        }
    } else if let Some(config) = model_config(session) {
        fn collect(options: &Value, result: &mut Vec<Value>) {
            if let Some(options) = options.as_array() {
                for option in options {
                    if let Some(id) = option["value"].as_str().filter(|id| !id.is_empty()) {
                        result.push(json!({"name":id,"display_name":option["name"].as_str().unwrap_or(id),"efforts":[]}));
                    } else {
                        collect(&option["options"], result);
                    }
                }
            }
        }
        collect(&config["options"], &mut result);
    }
    if result.is_empty() {
        return Err(
            "OpenCode returned no models. Run opencode auth login to configure a provider".into(),
        );
    }
    Ok(json!({"models":result}).to_string())
}

/// Convert the bounded, locally prepared CLI references to ACP content blocks.
fn acp_content(content: Value, image: bool) -> Result<Value, String> {
    let parts = content
        .as_array()
        .ok_or("Invalid prepared reference content")?;
    let mut result = Vec::with_capacity(parts.len());
    for part in parts {
        if part["type"] == "image" {
            if !image {
                return Err("This OpenCode CLI does not support image attachments".into());
            }
            result.push(json!({"type":"image","mimeType":part["source"]["media_type"],"data":part["source"]["data"]}));
        } else {
            result.push(part.clone());
        }
    }
    Ok(json!(result))
}

pub(crate) async fn list_models(prefs: &auris_session::AgentPreferences) -> Result<String, String> {
    let cwd = working_directory()?;
    let (mut cli, session) = tokio::time::timeout(
        STARTUP_TIMEOUT,
        Cli::spawn(prefs, cwd.path(), None, None, false),
    )
    .await
    .map_err(|_| "OpenCode startup timed out".to_string())??;
    let result = models(&session);
    cli.stop().await;
    result
}

pub(crate) async fn complete(
    prefs: &auris_session::AgentPreferences,
    prompt: &str,
) -> Result<String, String> {
    let cwd = working_directory()?;
    let (mut cli, session) = Cli::spawn(prefs, cwd.path(), None, None, false).await?;
    let result = async {
        cli.select_model(prefs, &session).await?;
        cli.prompt(json!([{"type":"text","text":prompt}]), None)
            .await
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
    mut cancelled: oneshot::Receiver<()>,
) -> Result<(), String> {
    let cwd = working_directory()?;
    let project = super::native_project_directory(history_path.as_deref());
    let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await?;
    let (mut cli, session) = tokio::select! {
        biased;
        _ = &mut cancelled => return Ok(()),
        result = tokio::time::timeout(STARTUP_TIMEOUT, Cli::spawn(prefs, cwd.path(), project, Some(&mcp), true)) => result.map_err(|_| "OpenCode startup timed out".to_string())??,
    };
    let result = tokio::select! {
        biased;
        _ = &mut cancelled => Ok(()),
        _ = mcp.cancelled() => Err("OpenCode cancelled a pending live tool request. Inspect the document before continuing".into()),
        result = async {
            cli.select_model(prefs, &session).await?;
            conversation(&mut cli, bridge, &mcp, history_path, fresh, history, &prefs.model).await
        } => result,
    };
    mcp.abort_calls();
    cli.stop().await;
    result
}

async fn conversation(
    cli: &mut Cli,
    bridge: &Bridge,
    mcp: &LiveMcp,
    history_path: Option<PathBuf>,
    fresh: bool,
    history: Option<HistorySnapshot>,
    model: &str,
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
    bridge.emit(json!({"event":"ready","model":model}));
    let mut context = super::memory_context(&memory);
    while let Some(line) = bridge.receive().await {
        let wire: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if wire["compact"] == true {
            bridge.emit(json!({"event":"compacted","ok":false,"manual":true,"message":"Manual compaction is not available for the OpenCode backend yet"}));
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
        // A leading slash belongs to the Auris request, never to OpenCode's
        // command dispatcher (which also runs before the model/tool loop).
        let prompt = format!("[Auris message]\n{context}\n{policy_context}\n{said}");
        let content = match crate::attachments::cli_input(prompt, attachments, bridge, true)
            .await
            .and_then(|content| {
                // Claude's fast path returns a string when there are no attachments.
                let content = if content.is_string() {
                    json!([{"type":"text","text":content}])
                } else {
                    content
                };
                acp_content(content, cli.image)
            }) {
            Ok(content) => content,
            Err(message) => {
                bridge.emit(json!({"event":"error","message":message}));
                continue;
            }
        };
        context.clear();
        mcp.set_active(true);
        let answer = cli.prompt(content, Some(bridge)).await;
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
        bridge.emit(json!({"event":"answer","text":answer?}));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    async fn reply(writer: &mut (impl AsyncWrite + Unpin), message: Value) {
        writer
            .write_all(format!("{message}\n").as_bytes())
            .await
            .unwrap();
    }

    fn transport() -> (Cli, tokio::io::DuplexStream) {
        let (client, server) = tokio::io::duplex(32768);
        let (reader, writer) = tokio::io::split(client);
        let mut cli = Cli::transport(None, writer, reader);
        cli.session = "session".into();
        cli.image = true;
        (cli, server)
    }

    fn notification(update: Value) -> Value {
        json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"session","update":update}})
    }

    #[test]
    fn catalogs_preserve_provider_and_variant_ids_and_support_grouped_options() {
        for session in [
            json!({"models":{"availableModels":[{"modelId":"provider/model/high","name":"Model (High)"},{"modelId":""}]}}),
            json!({"configOptions":[{"id":"model","category":"model","options":[{"group":"Provider","options":[{"value":"provider/model/high","name":"Model (High)"}]}]}]}),
        ] {
            let result: Value = serde_json::from_str(&models(&session).unwrap()).unwrap();
            assert_eq!(result["models"].as_array().unwrap().len(), 1);
            assert_eq!(result["models"][0]["name"], "provider/model/high");
            assert_eq!(result["models"][0]["display_name"], "Model (High)");
            assert_eq!(result["models"][0]["efforts"], json!([]));
        }
        assert!(
            models(&json!({}))
                .unwrap_err()
                .contains("opencode auth login")
        );
    }

    #[test]
    fn references_are_encoded_as_acp_images_without_changing_text() {
        let content = json!([{"type":"text","text":"Reference data"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"aW1hZ2U="}}]);
        let acp = acp_content(content.clone(), true).unwrap();
        assert_eq!(acp[0], content[0]);
        assert_eq!(
            acp[1],
            json!({"type":"image","mimeType":"image/png","data":"aW1hZ2U="})
        );
        assert!(
            acp_content(content, false)
                .unwrap_err()
                .contains("image attachments")
        );
    }

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
            super::super::Backend::OpenCode(_)
        ));
    }

    #[test]
    fn tool_free_configuration_uses_only_the_supplied_instructions() {
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
            assert_eq!(review[agents]["auris"][prompt], prefs.system_prompt);
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

    #[tokio::test]
    async fn initialization_passes_only_the_authenticated_live_server() {
        let (mut cli, server) = transport();
        cli.session.clear();
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
        let root = working_directory().unwrap();
        let peer = async {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            let init: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(init["method"], "initialize");
            assert_eq!(init["params"]["protocolVersion"], 1);
            assert_eq!(init["params"]["clientCapabilities"]["terminal"], false);
            reply(&mut writer, json!({"jsonrpc":"2.0","id":init["id"],"result":{"protocolVersion":1,"agentCapabilities":{"mcpCapabilities":{"http":true},"promptCapabilities":{"image":true}}}})).await;
            let new: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(new["method"], "session/new");
            assert!(Path::new(new["params"]["cwd"].as_str().unwrap()).is_absolute());
            let servers = new["params"]["mcpServers"].as_array().unwrap();
            assert_eq!(servers.len(), 1);
            assert_eq!(servers[0]["type"], "http");
            assert_eq!(servers[0]["url"], mcp.url);
            assert_eq!(
                servers[0]["headers"][0]["value"],
                format!("Bearer {}", mcp.token)
            );
            reply(&mut writer, json!({"jsonrpc":"2.0","id":new["id"],"result":{"sessionId":"created","models":{"availableModels":[{"modelId":"provider/model","name":"Model"}]},"modes":{"availableModes":[{"id":"auris"}]}}})).await;
            let mode: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(mode["method"], "session/set_mode");
            assert_eq!(mode["params"]["modeId"], "auris");
            reply(
                &mut writer,
                json!({"jsonrpc":"2.0","id":mode["id"],"result":{}}),
            )
            .await;
        };
        let (session, ()) = tokio::join!(cli.initialize(root.path(), Some(&mcp)), peer);
        models(&session.unwrap()).unwrap();
        assert_eq!(cli.session, "created");
        assert!(cli.image);
    }

    #[tokio::test]
    async fn streamed_text_reasoning_and_tool_results_are_not_duplicated() {
        let (mut cli, server) = transport();
        let (bridge, _commands, events) = crate::worker::test_bridge();
        let peer = async {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "session/prompt");
            let mut stale = notification(
                json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Wrong session"}}),
            );
            stale["params"]["sessionId"] = json!("old");
            reply(&mut writer, stale).await;
            reply(&mut writer, notification(json!({"sessionUpdate":"agent_thought_chunk","content":{"type":"text","text":"Planning"}}))).await;
            reply(&mut writer, notification(json!({"sessionUpdate":"tool_call","toolCallId":"call","title":"auris_live_add_track","rawInput":{},"status":"pending"}))).await;
            reply(&mut writer, notification(json!({"sessionUpdate":"tool_call_update","toolCallId":"call","rawInput":{"name":"Lead","kind":"instrument"},"status":"in_progress"}))).await;
            reply(&mut writer, notification(json!({"sessionUpdate":"tool_call_update","toolCallId":"call","title":"Added lead","status":"completed","content":[{"type":"content","content":{"type":"text","text":"Done"}},{"type":"content","content":{"type":"image","data":"do not store"}}]}))).await;
            for text in ["Added", "."] {
                reply(&mut writer, notification(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":text}}))).await;
            }
            reply(
                &mut writer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}}),
            )
            .await;
        };
        let (answer, ()) = tokio::join!(
            cli.prompt(json!([{"type":"text","text":"Compose"}]), Some(&bridge)),
            peer
        );
        assert_eq!(answer.unwrap(), "Added.");
        let events: Vec<_> = events.try_iter().collect();
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event"] == "call")
                .count(),
            1
        );
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "call" && event["args"]["name"] == "Lead")
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

    #[test]
    fn version_two_stream_keeps_the_tool_name_and_complete_arguments() {
        let (bridge, _commands, events) = crate::worker::test_bridge();
        let mut turn = Turn::default();
        turn.update(&json!({"sessionUpdate":"tool_call","toolCallId":"shell-id","name":"shell","title":"Run script","status":"pending","rawInput":{"cwd":"/project"}}), Some(&bridge)).unwrap();
        assert!(!events.try_iter().any(|event| event["event"] == "call"));
        turn.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"shell-id","title":"Inspect output","status":"in_progress","rawInput":{"command":"printf proof"}}), Some(&bridge)).unwrap();
        turn.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"shell-id","status":"completed"}), Some(&bridge)).unwrap();
        let events: Vec<_> = events.try_iter().collect();
        let call = events
            .iter()
            .find(|event| event["event"] == "call")
            .unwrap();
        assert_eq!(call["tool"], "shell");
        assert_eq!(
            call["args"],
            json!({"cwd":"/project","command":"printf proof"})
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event["event"] == "result")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn native_permissions_use_the_common_gate_and_never_select_allow_always() {
        for (allowed, stale) in [(true, false), (false, false), (true, true)] {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (mut cli, server) = transport();
                let (bridge, commands, events) = crate::worker::test_bridge();
                let peer = async {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();
                    let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    reply(&mut writer, json!({"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{
                        "sessionId":if stale { "old" } else { "session" },
                        "toolCall":{"toolCallId":"edit","title":"edit","kind":"edit","rawInput":{"filepath":"lead.txt","diff":"+melody"}},
                        "options":[{"optionId":"always","kind":"allow_always"},{"optionId":"once","kind":"allow_once"},{"optionId":"reject","kind":"reject_once"}]}})).await;
                    let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(response["id"], 900);
                    assert_eq!(response["result"]["outcome"]["optionId"], if allowed && !stale {"once"} else {"reject"});
                    reply(&mut writer, json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}})).await;
                };
                let owner = async {
                    if stale { return; }
                    loop {
                        match events.try_recv() {
                            Ok(event) if event["event"] == "permission" => {
                                assert_eq!(event["tool"], "cli_file_change");
                                assert_eq!(event["args"]["input"]["diff"], "+melody");
                                commands.send(json!({"event":"permission_result","id":event["id"],"ok":allowed}).to_string()).unwrap();
                                break;
                            }
                            Ok(_) | Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                            Err(error) => panic!("{error}"),
                        }
                    }
                };
                let (answer, (), ()) = tokio::join!(cli.prompt(json!([]), Some(&bridge)), peer, owner);
                answer.unwrap();
                if stale { assert!(!events.try_iter().any(|event| event["event"] == "permission")); }
            }).await.unwrap();
        }
    }

    #[tokio::test]
    async fn reviewer_refuses_permissions_and_unsupported_client_requests() {
        let (mut cli, server) = transport();
        let peer = async {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            reply(&mut writer, json!({"jsonrpc":"2.0","id":900,"method":"fs/write_text_file","params":{"sessionId":"session"}})).await;
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["error"]["code"], -32601);
            reply(&mut writer, json!({"jsonrpc":"2.0","id":901,"method":"session/request_permission","params":{"sessionId":"session","toolCall":{"kind":"execute"},"options":[{"kind":"allow_always","optionId":"always"}]}})).await;
            let response: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(response["result"]["outcome"]["outcome"], "cancelled");
            reply(
                &mut writer,
                json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"max_tokens"}}),
            )
            .await;
        };
        let (answer, ()) = tokio::join!(cli.prompt(json!([]), None), peer);
        assert!(answer.unwrap_err().contains("max_tokens"));
        let mut turn = Turn::default();
        assert!(
            turn.update(&json!({"sessionUpdate":"tool_call"}), None)
                .unwrap_err()
                .contains("tool")
        );
    }

    #[tokio::test]
    async fn model_selection_uses_the_advertised_acp_interface() {
        for (session, method, key) in [
            (
                json!({"models":{"availableModels":[]}}),
                "session/set_model",
                "modelId",
            ),
            (
                json!({"configOptions":[{"id":"model","category":"model"}]}),
                "session/set_config_option",
                "value",
            ),
        ] {
            let (mut cli, server) = transport();
            let prefs = auris_session::AgentPreferences {
                model: "provider/model/high".into(),
                ..Default::default()
            };
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                let request: Value =
                    serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["method"], method);
                assert_eq!(request["params"][key], prefs.model);
                reply(
                    &mut writer,
                    json!({"jsonrpc":"2.0","id":request["id"],"result":{}}),
                )
                .await;
            };
            let (result, ()) = tokio::join!(cli.select_model(&prefs, &session), peer);
            result.unwrap();
        }
    }

    #[tokio::test]
    async fn stop_sends_cancellation_before_closing_the_pipe() {
        let (mut cli, server) = transport();
        let peer = async {
            let mut lines = BufReader::new(server).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "session/cancel");
            assert_eq!(request["params"]["sessionId"], "session");
            assert!(lines.next_line().await.unwrap().is_none());
        };
        tokio::join!(cli.stop(), peer);
    }

    #[tokio::test]
    async fn acp_turn_edits_the_live_unsaved_song_and_undo_restores_it() {
        for allowed in [true, false] {
            tokio::time::timeout(Duration::from_secs(10), async {
            let (mut cli, server) = transport();
            let (bridge, commands, events) = crate::worker::test_bridge();
            let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
            commands.send(json!({"say":"/compose add an instrument track"}).to_string()).unwrap();
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                let prompt: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(prompt["method"], "session/prompt");
                let text = prompt["params"]["prompt"][0]["text"].as_str().unwrap();
                assert!(text.contains("/compose"));
                assert!(!text.trim_start().starts_with('/'), "Auris input must not invoke OpenCode slash commands");
                let client = reqwest::Client::new();
                let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token).header("Accept", "application/json, text/event-stream").json(&value);
                let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"acp-fixture","version":"1"}}})).send().await.unwrap();
                let session = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
                request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session).send().await.unwrap().error_for_status().unwrap();
                let result = request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add_track","arguments":{"name":"OpenCode Lead","kind":"instrument"}}})).header("mcp-session-id", &session).send().await.unwrap().text().await.unwrap();
                assert_eq!(result.contains("\"isError\":true"), !allowed, "{result}");
                reply(&mut writer, notification(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Added the track."}}))).await;
                reply(&mut writer, json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).await;
            };
            let owner = async {
                let mut song = auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
                let before = song.project().clone();
                let mut edits = 0;
                let mut permissions = 0;
                loop {
                    let event = match events.try_recv() {
                        Ok(event) => event,
                        Err(std::sync::mpsc::TryRecvError::Empty) => {tokio::task::yield_now().await; continue;}
                        Err(error) => panic!("{error}"),
                    };
                    match event["event"].as_str().unwrap_or_default() {
                        "permission" => {
                            permissions += 1;
                            commands.send(json!({"event":"permission_result","id":event["id"],"ok":allowed}).to_string()).unwrap();
                        }
                        "edit" => {
                            assert!(allowed);
                            edits += 1;
                            let text = song.agent_command(serde_json::from_value(event["command"].clone()).unwrap()).unwrap();
                            commands.send(json!({"event":"edit_result","ok":true,"text":text}).to_string()).unwrap();
                        }
                        "answer" => {assert_eq!(event["text"], "Added the track."); break;}
                        _ => {}
                    }
                }
                assert_eq!((permissions, edits), (1, usize::from(allowed)));
                assert_eq!(song.project().tracks.iter().any(|track| track.name == "OpenCode Lead"), allowed);
                assert!(song.path().is_none());
                if allowed {
                    assert!(song.is_dirty());
                    song.undo();
                }
                assert_eq!(song.project(), &before);
                drop(commands);
            };
            let (result, (), ()) = tokio::join!(conversation(&mut cli, &bridge, &mcp, None, false, None, "fixture"), peer, owner);
            result.unwrap();
        }).await.unwrap();
        }
    }

    #[tokio::test]
    async fn replacement_worker_replays_memory_once_and_checkpoints_display_text() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let root = working_directory().unwrap();
            let path = root.path().join("memory.json");
            let mut saved = memory::Memory::default();
            saved.push("Remember the melody", "Old response");
            saved.save(&path).unwrap();
            let (bridge, commands, _events) = crate::worker::test_bridge();
            let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
            let (mut cli, server) = transport();
            commands.send(json!({"say":"First request","display":"First display"}).to_string()).unwrap();
            commands.send(json!({"say":"Second request"}).to_string()).unwrap();
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                for index in 0..2 {
                    let prompt: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    let text = prompt["params"]["prompt"][0]["text"].as_str().unwrap();
                    assert_eq!(text.contains("Remember the melody"), index == 0);
                    let pending = memory::Memory::load(&path).unwrap();
                    assert_eq!(pending.turns.last().unwrap().user, if index == 0 { "First display" } else { "Second request" });
                    bridge.inspected_production_document(&mut json!({"revision":"fixture"})).unwrap();
                    assert_eq!(bridge.production_record(auris_session::production_record::Operation::Read).unwrap()["persistence"], "conversation");
                    reply(&mut writer, notification(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":format!("Response {index}")}}))).await;
                    reply(&mut writer, json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).await;
                }
                drop(commands);
            };
            let (result, ()) = tokio::join!(conversation(&mut cli, &bridge, &mcp, Some(path.clone()), false, None, "fixture"), peer);
            result.unwrap();
            let saved = memory::Memory::load(&path).unwrap();
            assert_eq!(saved.turns.len(), 3);
            assert_eq!(saved.turns[1].user, "First display");
            assert_eq!(saved.turns[2].answer, "Response 1");
        }).await.unwrap();
    }

    #[tokio::test]
    #[ignore = "requires an installed OpenCode CLI with a configured provider; no inference"]
    async fn installed_opencode_lists_models_and_opens_a_live_mcp_session() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
        let cwd = working_directory().unwrap();
        let prefs = auris_session::AgentPreferences::default();
        let (mut cli, session) = tokio::time::timeout(
            STARTUP_TIMEOUT,
            Cli::spawn(&prefs, cwd.path(), None, Some(&mcp), true),
        )
        .await
        .unwrap()
        .unwrap();
        models(&session).unwrap();
        cli.stop().await;
    }
}

#[cfg(test)]
mod evaluation;
