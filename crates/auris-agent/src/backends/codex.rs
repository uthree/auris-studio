//! Official Codex CLI integration; authentication and inference stay inside Codex.

use crate::{Bridge, HistorySnapshot, host::ToolHost, live_mcp::LiveMcp, memory};
use futures::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
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
// Turn notifications may echo base64 images from a bounded 25 MiB image/text turn.
const FRAME_LIMIT: usize = 40 * 1024 * 1024;

/// Separate process per conversation: settings, MCP connections and cancellation cannot leak.
struct Rpc {
    child: Option<Child>,
    input: Box<dyn AsyncWrite + Unpin + Send>,
    output: FramedRead<Box<dyn AsyncRead + Unpin + Send>, LinesCodec>,
    next_id: u64,
    thread: Option<String>,
    turn: Option<String>,
    deferred: VecDeque<Value>,
}

pub(crate) fn executable(prefs: &auris_session::AgentPreferences) -> Result<PathBuf, String> {
    if !prefs.codex_path.trim().is_empty() {
        let path = PathBuf::from(prefs.codex_path.trim());
        if cfg!(windows)
            && path
                .extension()
                .is_none_or(|extension| !extension.eq_ignore_ascii_case("exe"))
        {
            return Err(
                "Choose the native codex.exe executable, not a .cmd or .ps1 launcher".into(),
            );
        }
        return Ok(path);
    }
    for directory in std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()) {
        let path = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        if path.is_file() {
            return Ok(path);
        }
        // npm's Windows entry point is a shell shim. Launch its native payload directly.
        if cfg!(windows) {
            let triple = if cfg!(target_arch = "aarch64") {
                "aarch64-pc-windows-msvc"
            } else {
                "x86_64-pc-windows-msvc"
            };
            let package = if cfg!(target_arch = "aarch64") {
                "codex-win32-arm64"
            } else {
                "codex-win32-x64"
            };
            for vendor in [
                directory.join(format!(
                    "node_modules/@openai/codex/node_modules/@openai/{package}/vendor"
                )),
                directory.join("node_modules/@openai/codex/vendor"),
            ] {
                let path = vendor.join(triple).join("bin/codex.exe");
                if path.is_file() {
                    return Ok(path);
                }
            }
        }
    }
    Err("Codex CLI was not found. Install Codex, run codex login, or set its executable path in Agent settings".into())
}

fn command(
    prefs: &auris_session::AgentPreferences,
    cwd: &Path,
    overrides: &[String],
    token: Option<&str>,
) -> Result<Command, String> {
    let mut command = Command::new(executable(prefs)?);
    command
        .args(["app-server", "--listen", "stdio://"])
        .current_dir(cwd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // A child must not inherit the desktop's internal tool pipe, API credentials or routing.
    for (name, _) in std::env::vars_os() {
        let upper = name.to_string_lossy().to_ascii_uppercase();
        if (upper.starts_with("CODEX_") && upper != "CODEX_HOME") || upper.starts_with("OPENAI_") {
            command.env_remove(name);
        }
    }
    for setting in [
        "model_provider=\"openai\"",
        "approvals_reviewer=\"user\"",
        "features.shell_snapshot=false",
        "features.hooks=false",
        "features.plugins=false",
        "features.apps=false",
        "features.multi_agent=false",
        "features.code_mode=false",
        "features.remote_plugin=false",
        "features.memories=false",
        "features.skip_host_skill_discovery=true",
        "features.skill_mcp_dependency_install=false",
        "web_search=\"disabled\"",
        "project_doc_max_bytes=0",
    ] {
        command.args(["-c", setting]);
    }
    // Model discovery and independent approval review have no native execution tools.
    for feature in ["shell_tool", "unified_exec", "apply_patch_freeform"] {
        command.args(["-c", &format!("features.{feature}={}", token.is_some())]);
    }
    for setting in overrides {
        command.args(["-c", setting]);
    }
    command.args(["-c", "features.fast_mode=true"]);
    command.args([
        "-c",
        if prefs.fast_mode {
            "service_tier=\"fast\""
        } else {
            "service_tier=\"default\""
        },
    ]);
    if let Some(token) = token {
        command.env("AURIS_LIVE_MCP_TOKEN", token);
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000); // CREATE_NO_WINDOW; no interactive shell is involved.
    Ok(command)
}

impl Rpc {
    async fn spawn(
        prefs: &auris_session::AgentPreferences,
        cwd: &Path,
        overrides: &[String],
        token: Option<&str>,
    ) -> Result<Self, String> {
        let mut child = command(prefs, cwd, overrides, token)?
            .spawn()
            .map_err(|e| format!("Cannot start Codex CLI: {e}"))?;
        let input = child.stdin.take().ok_or("Codex stdin was not available")?;
        let output = child
            .stdout
            .take()
            .ok_or("Codex stdout was not available")?;
        let mut rpc = Self::transport(Some(child), input, output);
        rpc.request("initialize", json!({"clientInfo":{"name":"auris_studio", "title":"Auris Studio", "version":env!("CARGO_PKG_VERSION")}})).await?;
        rpc.send(json!({"method":"initialized"})).await?;
        let account = rpc
            .request("account/read", json!({"refreshToken":false}))
            .await?;
        require_chatgpt(&account)?;
        Ok(rpc)
    }

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
            thread: None,
            turn: None,
            deferred: VecDeque::new(),
        }
    }

    async fn connect(
        prefs: &auris_session::AgentPreferences,
        cwd: &Path,
        mcp: Option<&LiveMcp>,
    ) -> Result<Self, String> {
        tokio::time::timeout(STARTUP_TIMEOUT, async {
            let mut rpc = Self::spawn(prefs, cwd, &[], None).await?;
            let configuration = rpc.request("config/read", json!({})).await?;
            let mut overrides = Vec::new();
            // Empty TOML tables merge with inherited configuration. Disable each inherited
            // server explicitly, then restart before any thread or MCP connection is opened.
            if let Some(servers) = configuration["config"]["mcp_servers"].as_object()
                && !servers.is_empty()
            {
                // CLI override keys are split on dots without TOML key unquoting. Put
                // quoted server names inside the value so dots/quotes stay literal keys.
                let disabled = servers
                    .keys()
                    .map(|name| format!("{}={{enabled=false}}", json!(name)))
                    .collect::<Vec<_>>()
                    .join(",");
                overrides.push(format!("mcp_servers={{{disabled}}}"));
            }
            if let Some(mcp) = mcp {
                overrides.extend([
                    format!("mcp_servers.auris_live.url={}", json!(mcp.url)),
                    "mcp_servers.auris_live.enabled=true".into(),
                    "mcp_servers.auris_live.required=true".into(),
                    // Only this authenticated, application-owned endpoint delegates consent
                    // to ToolHost. Every call still reaches the window's live permission gate.
                    "mcp_servers.auris_live.default_tools_approval_mode=\"approve\"".into(),
                    "mcp_servers.auris_live.bearer_token_env_var=\"AURIS_LIVE_MCP_TOKEN\"".into(),
                ]);
            }
            if !overrides.is_empty() {
                rpc.stop().await;
                rpc =
                    Self::spawn(prefs, cwd, &overrides, mcp.map(|mcp| mcp.token.as_str())).await?;
            }
            Ok(rpc)
        })
        .await
        .map_err(|_| "Codex CLI startup timed out".to_string())?
    }

    async fn send(&mut self, message: Value) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
        encoded.push(b'\n');
        self.input
            .write_all(&encoded)
            .await
            .map_err(|e| format!("Codex connection closed: {e}"))
    }

    async fn read(&mut self) -> Result<Value, String> {
        let line = self
            .output
            .next()
            .await
            .ok_or("Codex CLI exited or closed its connection")?
            .map_err(|e| e.to_string())?;
        serde_json::from_str(&line).map_err(|e| format!("Invalid Codex protocol message: {e}"))
    }

    async fn reject_request(&mut self, message: &Value) -> Result<(), String> {
        if message.get("id").is_some() && message.get("method").is_some() {
            self.send(json!({"id":message["id"],"error":{"code":-32601,"message":"Unsupported or out-of-turn Codex request"}})).await?;
        }
        Ok(())
    }

    async fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        tokio::time::timeout(STARTUP_TIMEOUT, self.request_inner(method, params))
            .await
            .map_err(|_| format!("Codex {method} timed out"))?
    }

    async fn request_inner(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"id":id,"method":method,"params":params}))
            .await?;
        loop {
            let message = self.read().await?;
            if message["id"] == id && message.get("method").is_none() {
                return if let Some(error) = message.get("error") {
                    Err(format!(
                        "Codex {method}: {}",
                        error["message"].as_str().unwrap_or("protocol error")
                    ))
                } else {
                    Ok(message["result"].clone())
                };
            }
            if method == "turn/start"
                && message.get("id").is_some()
                && message.get("method").is_some()
            {
                // Approval can precede the turn/start response. Handle it only after the
                // returned turn ID is available for conversation-scope validation.
                if self.deferred.len() >= 1024 {
                    return Err("Codex sent too many pending requests".into());
                }
                self.deferred.push_back(message);
                continue;
            }
            self.reject_request(&message).await?;
            if message.get("method").is_some() && message.get("id").is_none() {
                if self.deferred.len() >= 1024 {
                    return Err("Codex sent too many pending notifications".into());
                }
                self.deferred.push_back(message);
            }
        }
    }

    async fn stop(&mut self) {
        if let (Some(thread), Some(turn)) = (&self.thread, &self.turn) {
            self.next_id += 1;
            let _ = self.send(json!({"id":self.next_id,"method":"turn/interrupt","params":{"threadId":thread,"turnId":turn}})).await;
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                while let Ok(message) = self.read().await {
                    if message["method"] == "turn/completed" {
                        break;
                    }
                }
            })
            .await;
        }
        let _ = self.input.shutdown().await;
        if let Some(child) = self.child.as_mut()
            && tokio::time::timeout(Duration::from_secs(2), child.wait())
                .await
                .is_err()
        {
            let _ = child.kill().await;
        }
    }
}

fn require_chatgpt(account: &Value) -> Result<(), String> {
    if account["account"]["type"] == "chatgpt" {
        Ok(())
    } else {
        Err("Codex requires ChatGPT sign-in. Run codex login before using this backend; API-key authentication is not used".into())
    }
}

pub(crate) async fn complete(
    prefs: &auris_session::AgentPreferences,
    prompt: &str,
) -> Result<String, String> {
    let cwd = tempfile::tempdir().map_err(|error| error.to_string())?;
    let mut rpc = Rpc::connect(prefs, cwd.path(), None).await?;
    let result = async {
        let thread = rpc
            .request(
                "thread/start",
                json!({"model":prefs.model,"cwd":cwd.path(),"ephemeral":true,
            "sandbox":"read-only","approvalPolicy":"never","baseInstructions":prefs.system_prompt}),
            )
            .await?;
        let id = thread["thread"]["id"]
            .as_str()
            .ok_or("Missing text-only thread")?
            .to_owned();
        rpc.thread = Some(id.clone());
        let turn = rpc
            .request(
                "turn/start",
                json!({"threadId":id,"input":[{"type":"text","text":prompt}]}),
            )
            .await?;
        rpc.turn = turn["turn"]["id"].as_str().map(str::to_owned);
        let mut answer = String::new();
        loop {
            let message = match rpc.deferred.pop_front() {
                Some(message) => message,
                None => rpc.read().await?,
            };
            rpc.reject_request(&message).await?;
            let params = &message["params"];
            if params["threadId"]
                .as_str()
                .is_some_and(|thread| thread != id)
            {
                continue;
            }
            match message["method"].as_str() {
                Some("item/agentMessage/delta") => {
                    answer.push_str(params["delta"].as_str().unwrap_or_default())
                }
                Some("item/completed")
                    if params["item"]["type"] == "agentMessage" && answer.is_empty() =>
                {
                    answer = params["item"]["text"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned()
                }
                Some("item/started")
                    if !matches!(
                        params["item"]["type"].as_str(),
                        Some("reasoning" | "agentMessage" | "userMessage")
                    ) =>
                {
                    return Err("Text-only agent attempted to use a tool".into());
                }
                Some("turn/completed") => {
                    rpc.turn = None;
                    return if params["turn"]["status"] == "completed" {
                        Ok(answer)
                    } else {
                        Err("Text-only agent failed".into())
                    };
                }
                _ => {}
            }
            if answer.len() > 8192 {
                return Err("Text-only response was too large".into());
            }
        }
    }
    .await;
    rpc.stop().await;
    result
}

pub(crate) async fn list_models(prefs: &auris_session::AgentPreferences) -> Result<String, String> {
    let cwd = tempfile::tempdir().map_err(|e| e.to_string())?;
    let mut rpc = Rpc::connect(prefs, cwd.path(), None).await?;
    let mut models = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..10 {
        let page = rpc
            .request("model/list", json!({"cursor":cursor,"limit":100}))
            .await?;
        if let Some(data) = page["data"].as_array() {
            models.extend(
                data.iter()
                    .filter_map(|model| model["model"].as_str().map(|name| json!({
                        "name":name,
                        "display_name":model["displayName"],
                        "efforts":model["supportedReasoningEfforts"].as_array().map(|efforts| efforts.iter().filter_map(|effort| effort["reasoningEffort"].as_str()).collect::<Vec<_>>())
                    }))),
            );
        }
        cursor = page["nextCursor"].clone();
        if cursor.is_null() {
            break;
        }
    }
    if !cursor.is_null() {
        return Err("Codex returned too many model pages".into());
    }
    rpc.stop().await;
    Ok(json!({"models":models}).to_string())
}

pub(crate) async fn run(
    prefs: &auris_session::AgentPreferences,
    bridge: &Bridge,
    history_path: Option<PathBuf>,
    fresh: bool,
    history: Option<HistorySnapshot>,
    cancelled: oneshot::Receiver<()>,
) -> Result<(), String> {
    // Keep CLI configuration private; the model's working directory is the saved project.
    let working = tempfile::tempdir().map_err(|e| e.to_string())?;
    let cwd = super::native_project_directory(history_path.as_deref())
        .unwrap_or(working.path())
        .to_path_buf();
    let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await?;
    let mut cancelled = cancelled;
    let mut rpc = tokio::select! {
        biased;
        _ = &mut cancelled => return Ok(()),
        result = Rpc::connect(prefs, working.path(), Some(&mcp)) => result?,
    };
    let result = tokio::select! {
        biased;
        _ = &mut cancelled => Ok(()),
        _ = mcp.cancelled() => Err("Codex cancelled a pending live tool request. Inspect the document before starting another conversation".into()),
        result = conversation(&mut rpc, prefs, bridge, &mcp, &cwd, HistoryInput { path: history_path, fresh, snapshot: history }) => result,
    };
    mcp.abort_calls();
    rpc.stop().await;
    result
}

struct HistoryInput {
    path: Option<PathBuf>,
    fresh: bool,
    snapshot: Option<HistorySnapshot>,
}

async fn conversation(
    rpc: &mut Rpc,
    prefs: &auris_session::AgentPreferences,
    bridge: &Bridge,
    mcp: &LiveMcp,
    cwd: &Path,
    history: HistoryInput,
) -> Result<(), String> {
    let preloaded = history.snapshot.is_some();
    let (mut memory, mut generation) = match (history.snapshot, history.path) {
        (Some(snapshot), _) => {
            let (memory, generation) = snapshot.into_parts();
            (memory, Some(generation))
        }
        (None, Some(path)) => {
            let (memory, generation) = bridge.open_history(&path, history.fresh)?;
            (memory, Some(generation))
        }
        _ => (memory::Memory::default(), None),
    };
    bridge.bind_production_history(generation.as_ref())?;
    if !preloaded {
        bridge.emit(json!({"event":"history","summary":memory.summary,"turns":memory.turns}));
    }
    let thread = rpc.request("thread/start", json!({"model":prefs.model,"cwd":cwd,"ephemeral":true,"sandbox":"read-only","approvalPolicy":"untrusted","baseInstructions":super::cli_preamble(prefs)})).await?;
    let thread_id = thread["thread"]["id"]
        .as_str()
        .ok_or("Codex did not return a thread ID")?
        .to_owned();
    rpc.thread = Some(thread_id.clone());
    // Check the effective catalog rather than assuming overrides were honored by this CLI.
    let servers = rpc
        .request(
            "mcpServerStatus/list",
            json!({"threadId":thread_id,"limit":100}),
        )
        .await?;
    validate_servers(&servers)?;
    bridge.emit(json!({"event":"ready","model":prefs.model}));
    let mut context = super::memory_context(&memory);
    while let Some(line) = bridge.receive().await {
        let wire: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
        if wire["compact"] == true {
            bridge.emit(json!({"event":"compacted","ok":false,"manual":true,"message":"Manual compaction is not available for the Codex backend yet"}));
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
        let input = match crate::attachments::cli_input(prompt, attachments, bridge, false).await {
            Ok(input) => input,
            Err(message) => {
                bridge.emit(json!({"event":"error","message":message}));
                continue;
            }
        };
        context.clear();
        mcp.set_active(true);
        let turn = rpc.request("turn/start", json!({"threadId":thread_id,"input":input,"effort":if prefs.effort == auris_session::ReasoningEffort::Default { Value::Null } else { json!(prefs.effort.as_str()) }})).await?;
        rpc.turn = turn["turn"]["id"].as_str().map(str::to_owned);
        let answer = receive_turn(rpc, bridge, &thread_id).await;
        mcp.set_active(false);
        rpc.turn = None;
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
            Err(message) => return Err(message),
        }
    }
    Ok(())
}

fn validate_servers(servers: &Value) -> Result<(), String> {
    if !servers["nextCursor"].is_null() {
        return Err("Cannot verify the complete Codex MCP catalog".into());
    }
    let servers = servers["data"]
        .as_array()
        .ok_or("Codex did not return its MCP catalog")?;
    let mut live_found = false;
    for server in servers {
        let count = server["tools"].as_object().map_or(0, |tools| tools.len());
        if server["name"] == "auris_live" {
            live_found = count > 0;
        } else if count > 0 {
            return Err(
                "Codex exposed an unrelated MCP server; refusing to start the conversation".into(),
            );
        }
    }
    if live_found {
        Ok(())
    } else {
        Err("Codex could not connect to the Auris live MCP tools".into())
    }
}

async fn receive_turn(rpc: &mut Rpc, bridge: &Bridge, thread: &str) -> Result<String, String> {
    let mut answer = String::new();
    let mut native_items = std::collections::HashMap::<String, Value>::new();
    loop {
        let message = match rpc.deferred.pop_front() {
            Some(message) => message,
            None => rpc.read().await?,
        };
        let params = &message["params"];
        if message.get("id").is_some() && message.get("method").is_some() {
            let operation = match message["method"].as_str() {
                Some("item/commandExecution/requestApproval") => Some("cli_command"),
                Some("item/fileChange/requestApproval") => Some("cli_file_change"),
                _ => None,
            };
            if let Some(operation) = operation
                && params["threadId"] == thread
                && params["turnId"].as_str() == rpc.turn.as_deref()
                && rpc.turn.is_some()
            {
                let _guard = bridge.tool_lock().await;
                let item = params["itemId"]
                    .as_str()
                    .and_then(|id| native_items.get(id));
                let origin = crate::permissions::call_origin(
                    params["itemId"].as_str(),
                    operation,
                    "codex_approval",
                );
                let result = crate::permissions::with_origin(
                    origin,
                    crate::permissions::authorize(
                        Some(bridge),
                        operation,
                        &json!({"backend":"codex","request":params,"item":item}).to_string(),
                    ),
                )
                .await;
                rpc.send(json!({"id":message["id"],"result":{"decision":if result.is_ok() { "accept" } else { "decline" }}})).await?;
            } else {
                rpc.reject_request(&message).await?;
            }
            continue;
        }
        if params["threadId"].as_str().is_some_and(|id| id != thread) {
            continue;
        }
        if params["turnId"]
            .as_str()
            .is_some_and(|id| Some(id) != rpc.turn.as_deref())
        {
            continue;
        }
        match message["method"].as_str().unwrap_or_default() {
            "item/started" | "item/completed"
                if matches!(
                    params["item"]["type"].as_str(),
                    Some("commandExecution" | "fileChange")
                ) =>
            {
                let item = &params["item"];
                let tool = if item["type"] == "commandExecution" {
                    "cli_command"
                } else {
                    "cli_file_change"
                };
                if message["method"] == "item/started" {
                    if let Some(id) = item["id"].as_str() {
                        native_items.insert(id.to_owned(), item.clone());
                    }
                    bridge.emit(json!({"event":"phase","phase":"tool"}));
                    bridge
                        .emit(json!({"event":"call","call_id":item["id"],"tool":tool,"args":item}));
                } else {
                    if let Some(id) = item["id"].as_str() {
                        native_items.remove(id);
                    }
                    let text = item["aggregatedOutput"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| item.to_string());
                    bridge.emit(json!({"event":"result","call_id":item["id"],"tool":tool,"ok":item["status"] == "completed" && item["exitCode"].as_i64().is_none_or(|code| code == 0),"text":text}));
                }
            }
            "item/agentMessage/delta" => {
                let text = params["delta"].as_str().unwrap_or_default();
                answer.push_str(text);
                bridge.emit(json!({"event":"text_delta","text":text}));
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                bridge.emit(json!({"event":"reasoning_delta","text":params["delta"]}))
            }
            "item/completed" if params["item"]["type"] == "agentMessage" && answer.is_empty() => {
                answer = params["item"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
            }
            "item/started" | "item/completed" if params["item"]["type"] == "mcpToolCall" => {
                let item = &params["item"];
                if message["method"] == "item/started" {
                    bridge.emit(json!({"event":"phase","phase":"tool"}));
                    bridge.emit(json!({"event":"call","call_id":item["id"],"tool":item["tool"],"args":item["arguments"]}));
                } else {
                    // Images belong to the model input, not to the transcript's text details.
                    let text = item["error"]["message"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            item["result"]["content"]
                                .as_array()
                                .map(|content| {
                                    content
                                        .iter()
                                        .filter_map(|part| part["text"].as_str())
                                        .collect::<Vec<_>>()
                                        .join("\n")
                                })
                                .unwrap_or_default()
                        });
                    bridge.emit(json!({"event":"result","call_id":item["id"],"tool":item["tool"],"ok":item["status"] == "completed" && item["error"].is_null() && item["result"]["isError"] != true,"text":text}));
                }
            }
            "turn/completed" => {
                if params["turn"]["id"].as_str() != rpc.turn.as_deref() {
                    continue;
                }
                return match params["turn"]["status"].as_str() {
                    Some("completed") => Ok(answer),
                    Some("interrupted") => {
                        Err("Codex turn interrupted; inspect the document before continuing".into())
                    }
                    _ => Err(format!(
                        "Codex turn failed: {}",
                        params["turn"]["error"]["message"]
                            .as_str()
                            .unwrap_or("unknown error")
                    )),
                };
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, BufReader};

    #[test]
    fn fast_mode_overrides_inherited_service_tiers_in_both_directions() {
        let cwd = tempfile::tempdir().unwrap();
        for fast_mode in [false, true] {
            let prefs = auris_session::AgentPreferences {
                codex_path: if cfg!(windows) {
                    "C:/Tools/codex.exe"
                } else {
                    "/usr/local/bin/codex"
                }
                .into(),
                fast_mode,
                ..Default::default()
            };
            let command = command(&prefs, cwd.path(), &[], None).unwrap();
            let args: Vec<_> = command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy())
                .collect();
            let tier = if fast_mode { "fast" } else { "default" };
            assert!(
                args.iter()
                    .any(|arg| *arg == format!("service_tier=\"{tier}\""))
            );
            assert!(args.iter().any(|arg| *arg == "features.fast_mode=true"));
        }
    }

    #[test]
    fn only_the_live_worker_enables_native_execution_tools() {
        let cwd = tempfile::tempdir().unwrap();
        let prefs = auris_session::AgentPreferences {
            codex_path: if cfg!(windows) {
                "C:/Tools/codex.exe"
            } else {
                "/usr/local/bin/codex"
            }
            .into(),
            ..Default::default()
        };
        for token in [None, Some("private-token")] {
            let command = command(&prefs, cwd.path(), &[], token).unwrap();
            let args: Vec<_> = command
                .as_std()
                .get_args()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect();
            for feature in ["shell_tool", "unified_exec", "apply_patch_freeform"] {
                assert!(args.contains(&format!("features.{feature}={}", token.is_some())));
            }
            assert!(args.contains(&"approvals_reviewer=\"user\"".into()));
            assert!(!args.iter().any(|arg| arg.contains("private-token")));
        }
    }

    #[test]
    fn subscription_backend_rejects_api_keys_and_signed_out_accounts() {
        assert!(require_chatgpt(&json!({"account":{"type":"chatgpt"}})).is_ok());
        assert!(require_chatgpt(&json!({"account":{"type":"apiKey"}})).is_err());
        assert!(require_chatgpt(&json!({"account":null})).is_err());
    }

    #[test]
    fn unrelated_mcp_tools_and_missing_live_tools_refuse_startup() {
        let live = json!({"name":"auris_live","tools":{"add_track":{}}});
        assert!(validate_servers(&json!({"data":[live.clone()],"nextCursor":null})).is_ok());
        assert!(
            validate_servers(&json!({"data":[live,{"name":"other","tools":{"shell":{}}}]}))
                .is_err()
        );
        assert!(validate_servers(&json!({"data":[]})).is_err());
    }

    async fn reply(writer: &mut (impl AsyncWrite + Unpin), value: Value) {
        writer
            .write_all(format!("{value}\n").as_bytes())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn native_approvals_include_the_diff_and_reject_other_turns() {
        for (allowed, stale) in [(true, false), (false, false), (true, true)] {
            tokio::time::timeout(Duration::from_secs(5), async {
                let (bridge, commands, events) = crate::worker::test_bridge();
                let (client, server) = tokio::io::duplex(32768);
                let (reader, writer) = tokio::io::split(client);
                let mut rpc = Rpc::transport(None, writer, reader);
                rpc.turn = Some("turn".into());
                let item = json!({"id":"edit-1","type":"fileChange","status":"inProgress","changes":[{"path":"export.py","diff":"+print('export')"}]});
                let peer = async {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();
                    reply(&mut writer, json!({"method":"item/started","params":{"threadId":"thread","turnId":"turn","item":item}})).await;
                    reply(&mut writer, json!({"id":900,"method":"item/fileChange/requestApproval","params":{"threadId":"thread","turnId":if stale { "old" } else { "turn" },"itemId":"edit-1"}})).await;
                    let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(response["id"], 900);
                    if stale { assert!(response.get("error").is_some()); }
                    else { assert_eq!(response["result"]["decision"], if allowed { "accept" } else { "decline" }); }
                    reply(&mut writer, json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed"}}})).await;
                };
                let owner = async {
                    if stale { return; }
                    loop {
                        match events.try_recv() {
                            Ok(event) if event["event"] == "permission" => {
                                assert_eq!(event["tool"], "cli_file_change");
                                assert_eq!(event["args"]["item"], item);
                                commands.send(json!({"event":"permission_result","id":event["id"],"ok":allowed,"reason":"fixture"}).to_string()).unwrap();
                                break;
                            }
                            Ok(_) | Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                            Err(error) => panic!("{error}"),
                        }
                    }
                };
                let (result, (), ()) = tokio::join!(receive_turn(&mut rpc, &bridge, "thread"), peer, owner);
                result.unwrap();
                if stale { assert!(!events.try_iter().any(|event| event["event"] == "permission")); }
            }).await.unwrap();
        }
    }

    /// Real MCP HTTP traffic and real session commands, with a scripted Codex RPC peer.
    #[tokio::test]
    async fn codex_mcp_imports_external_notes_and_undo_restores_the_unsaved_song() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
            let (client, server) = tokio::io::duplex(32768);
            let (reader, writer) = tokio::io::split(client);
            let mut rpc = Rpc::transport(None, writer, reader);
            let prefs = auris_session::AgentPreferences { backend:"codex".into(), model:"fixture".into(), system_prompt:"Write a second verse.\nKeep the motif.".into(), ..Default::default() };
            let cwd = tempfile::tempdir().unwrap();
            let audio = cwd.path().join("reference.wav");
            let history = cwd.path().join("memory.json");
            crate::attachments::tests::reference_wav(&audio, false);
            commands.send(json!({"say":"Add an instrument track", "display":"Add a track", "attachments":[audio]}).to_string()).unwrap();
            let drive = async {
                conversation(&mut rpc, &prefs, &bridge, &mcp, cwd.path(), HistoryInput { path: Some(history.clone()), fresh: false, snapshot: None }).await.unwrap();
            };
            let peer = async {
                let (reader, mut writer) = tokio::io::split(server);
                let mut lines = BufReader::new(reader).lines();
                let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["params" ]["baseInstructions"], super::super::cli_preamble(&prefs));
                assert_eq!(request["method"], "thread/start");
                assert_eq!(request["params"]["sandbox"], "read-only");
                reply(&mut writer, json!({"id":request["id"],"result":{"thread":{"id":"thread"}}})).await;
                let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["method"], "mcpServerStatus/list");
                reply(&mut writer, json!({"id":request["id"],"result":{"data":[{"name":"auris_live","tools":{"import_transcription":{}}}]}})).await;
                let request: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                assert_eq!(request["method"], "turn/start");
                let checkpoint = memory::Memory::load(&history).unwrap();
                assert_eq!(checkpoint.turns.last().unwrap().user, "Add a track");
                assert!(checkpoint.turns.last().unwrap().answer.contains("no final answer was recorded"));
                assert!(request["params"]["input"][1]["text"].as_str().unwrap().contains("local CPU audio measurements"));

                reply(&mut writer, json!({"id":request["id"],"result":{"turn":{"id":"turn"}}})).await;

                let client = reqwest::Client::new();
                let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token).header("Accept", "application/json, text/event-stream").json(&value);
                assert_eq!(client.post(&mcp.url).json(&json!({})).send().await.unwrap().status(), reqwest::StatusCode::UNAUTHORIZED);
                let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}})).send().await.unwrap();
                assert!(init.status().is_success());
                let session = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
                let notify = request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session).send().await.unwrap();
                assert!(notify.status().is_success());
                let result=crate::live_mcp::transcription_test_call(&client,&mcp.url,&mcp.token,&session,"Codex lead").await;
                assert_ne!(result["result"]["isError"], true, "{result}");
                assert!(result["error"].is_null(), "{result}");
                reply(&mut writer, json!({"method":"item/agentMessage/delta","params":{"threadId":"thread","turnId":"turn","delta":"Added the track."}})).await;
                reply(&mut writer, json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"completed"}}})).await;
            };
            let owner = async {
                let mut session = auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
                let before = session.project().clone();
                let mut permissions = 0;
                let mut edits = 0;
                loop {
                    let event = match events.try_recv() {
                        Ok(event) => event,
                        Err(std::sync::mpsc::TryRecvError::Empty) => { tokio::time::sleep(Duration::from_millis(1)).await; continue; }
                        Err(error) => panic!("{error}"),
                    };
                    match event["event"].as_str().unwrap_or_default() {
                        "permission" => {
                            permissions += 1;
                            let operation = auris_session::agent_policy::Operation::parse(event["tool"].as_str().unwrap(), &event["args"]).unwrap();
                            assert_eq!(auris_session::agent_policy::Policy::default().decide(&operation), auris_session::agent_policy::Decision::Allow);
                            commands.send(json!({"event":"permission_result","id":event["id"],"ok":true}).to_string()).unwrap();
                        }
                        "edit" => {
                            edits += 1;
                            let command = serde_json::from_value(event["command"].clone()).unwrap();
                            assert!(event["command"]["draft"]["transcription"]["notes"].as_array().unwrap().len() >= 3);
                            let text = session.agent_command(command).unwrap();
                            commands.send(json!({"event":"edit_result","ok":true,"text":text}).to_string()).unwrap();
                        }
                        "answer" => {
                            assert_eq!(event["text"], "Added the track.");
                            break;
                        }
                        _ => {}
                    }
                }
                assert_eq!((permissions, edits), (1, 1));
                assert!(session.project().tracks.iter().any(|track| track.name == "Codex lead"));
                let piano = session.project().tracks.iter().find(|track|track.name == "Codex lead").unwrap();
                assert_eq!(piano.kind.as_instrument().unwrap().instrument_id, "auris.physical.piano");
                assert!(session.path().is_none());
                assert!(session.is_dirty());
                session.undo();
                assert_eq!(session.project(), &before);
                drop(commands);
            };
            let ((), (), ()) = tokio::join!(drive, peer, owner);
        }).await.unwrap();
    }

    #[tokio::test]
    async fn failed_and_wrong_turn_completion_cannot_report_success() {
        let (bridge, _commands, events) = crate::worker::test_bridge();
        let (client, mut server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut rpc = Rpc::transport(None, writer, reader);
        rpc.turn = Some("active".into());
        reply(&mut server, json!({"method":"item/completed","params":{"threadId":"thread","turnId":"active","item":{"id":"call","type":"mcpToolCall","tool":"add_track","status":"completed","error":null,"result":{"isError":true,"content":[{"type":"text","text":"Permission denied"}]}}}})).await;
        reply(&mut server, json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"old","status":"completed"}}})).await;
        reply(&mut server, json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"active","status":"failed","error":{"message":"fixture failure"}}}})).await;
        assert!(
            receive_turn(&mut rpc, &bridge, "thread")
                .await
                .unwrap_err()
                .contains("fixture failure")
        );
        let result = events.try_recv().unwrap();
        assert_eq!(result["event"], "result");
        assert_eq!(result["ok"], false);
        assert_eq!(result["text"], "Permission denied");
    }

    #[tokio::test]
    async fn notifications_before_rpc_responses_are_preserved() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut rpc = Rpc::transport(None, writer, reader);
        reply(
            &mut server,
            json!({"method":"item/agentMessage/delta","params":{"delta":"early"}}),
        )
        .await;
        reply(&mut server, json!({"id":1,"result":{"turn":{"id":"turn"}}})).await;
        rpc.request("turn/start", json!({})).await.unwrap();
        assert_eq!(
            rpc.deferred.pop_front().unwrap()["params"]["delta"],
            "early"
        );
    }

    #[tokio::test]
    async fn cancellation_sends_a_turn_interrupt_before_closing() {
        let (client, server) = tokio::io::duplex(4096);
        let (reader, writer) = tokio::io::split(client);
        let mut rpc = Rpc::transport(None, writer, reader);
        rpc.thread = Some("thread".into());
        rpc.turn = Some("turn".into());
        let peer = async {
            let (reader, mut writer) = tokio::io::split(server);
            let mut lines = BufReader::new(reader).lines();
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "turn/interrupt");
            assert_eq!(
                request["params"],
                json!({"threadId":"thread","turnId":"turn"})
            );
            reply(&mut writer, json!({"method":"turn/completed","params":{"threadId":"thread","turn":{"id":"turn","status":"interrupted"}}})).await;
        };
        let ((), ()) = tokio::join!(rpc.stop(), peer);
    }

    #[tokio::test]
    #[ignore = "requires an installed Codex CLI signed in with ChatGPT; no inference"]
    async fn installed_codex_connects_to_the_live_mcp_catalog() {
        let (bridge, _commands, _events) = crate::worker::test_bridge();
        let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
        let prefs = auris_session::AgentPreferences {
            fast_mode: true,
            ..Default::default()
        };
        let cwd = tempfile::tempdir().unwrap();
        let mut rpc = Rpc::connect(&prefs, cwd.path(), Some(&mcp)).await.unwrap();
        let thread = rpc.request("thread/start", json!({"cwd":cwd.path(),"ephemeral":true,"sandbox":"read-only","approvalPolicy":"on-request","baseInstructions":crate::preamble()})).await.unwrap();
        // Some CLI versions canonicalize the renamed fast tier to its original API name.
        assert!(matches!(
            thread["serviceTier"].as_str(),
            Some("fast" | "priority")
        ));
        let status = rpc
            .request(
                "mcpServerStatus/list",
                json!({"threadId":thread["thread"]["id"],"limit":100}),
            )
            .await
            .unwrap();
        validate_servers(&status).unwrap();
        let live = status["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["name"] == "auris_live")
            .unwrap();
        assert!(live["tools"].get("generate_phrase").is_some(), "{status}");
        assert!(
            live["tools"].get("inspect_performance").is_some(),
            "{status}"
        );
        assert!(live["tools"].get("set_performance").is_some(), "{status}");
        rpc.stop().await;
    }

    #[tokio::test]
    #[ignore = "runs one real Codex turn using the signed-in ChatGPT subscription"]
    async fn installed_codex_edits_an_unsaved_song() {
        let prefs = auris_session::AgentPreferences {
            backend: "codex".into(),
            model: "gpt-6.1-sol".into(),
            system_prompt: "You edit the open Auris document. For the prompt settings smoke check, call inspect_project, then add_track exactly once with name Codex custom prompt lead and kind instrument. Do not change anything else or use native tools. No notes or rendering are needed. Report completion.".into(),
            ..Default::default()
        };
        let listing: Value = serde_json::from_str(&list_models(&prefs).await.unwrap()).unwrap();
        assert!(
            listing["models"]
                .as_array()
                .unwrap()
                .iter()
                .any(|model| model["name"] == prefs.model)
        );
        let mut worker = crate::Worker::spawn(prefs, None, true, None).unwrap();
        worker.send(&json!({"say":"Run the prompt settings smoke check from your system instructions.","policy":auris_session::agent_policy::Policy::default()}).to_string()).unwrap();
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let before = session.project().clone();
        let deadline = std::time::Instant::now() + Duration::from_secs(120);
        loop {
            assert!(
                std::time::Instant::now() < deadline,
                "Codex smoke test timed out"
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
                    let allowed = matches!(
                        operation.name.as_str(),
                        "edit_project.inspect" | "edit_project.add_track"
                    );
                    worker
                        .send(
                            &json!({"event":"permission_result","id":event["id"],"ok":allowed})
                                .to_string(),
                        )
                        .unwrap();
                }
                "edit" => {
                    let action = event["command"]["action"].as_str().unwrap();
                    assert!(matches!(action, "inspect" | "add_track"));
                    let result = session
                        .agent_command(serde_json::from_value(event["command"].clone()).unwrap());
                    worker.send(&json!({"event":"edit_result","ok":result.is_ok(),"text":result.unwrap_or_else(|error| error)}).to_string()).unwrap();
                }
                "error" => panic!("{}", event["message"]),
                "ended" => panic!("Codex ended before answering"),
                "answer" => break,
                _ => {}
            }
        }
        assert!(
            session
                .project()
                .tracks
                .iter()
                .any(|track| track.name == "Codex custom prompt lead")
        );
        assert_eq!(session.project().tracks.len(), before.tracks.len() + 1);
        assert!(session.path().is_none());
        assert!(session.is_dirty());
        session.undo();
        assert_eq!(session.project(), &before);
        worker.cancel();
    }
}
