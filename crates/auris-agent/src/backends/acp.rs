//! Shared ACP runtime. The Rust SDK owns JSON-RPC dispatch and response ordering.

use super::acp_process::ProcessScope;
use crate::{Bridge, HistorySnapshot, host::ToolHost, live_mcp::LiveMcp, memory};
use agent_client_protocol::schema::v1::{
    RequestPermissionOutcome, RequestPermissionRequest, RequestPermissionResponse,
};
use agent_client_protocol::{Agent, Client, ConnectionTo, Lines, Responder, UntypedMessage};
use futures::StreamExt;
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, oneshot},
};
use tokio_util::codec::{FramedRead, LinesCodec};

const STARTUP_TIMEOUT: Duration = Duration::from_secs(30);
const FRAME_LIMIT: usize = 40 * 1024 * 1024;
mod recovery;
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
    "Bash",
    "Read",
    "Glob",
    "Grep",
    "Write",
    "Edit",
    "NotebookEdit",
    "shell_command",
    "exec_command",
    "write_stdin",
];

fn working_directory() -> Result<tempfile::TempDir, String> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("auris-acp-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(std::fs::Permissions::from_mode(0o700));
    }
    builder.tempdir().map_err(|error| error.to_string())
}

/// A cancelled permission future must close its request without granting an operation.
struct PermissionReply(Option<Responder<RequestPermissionResponse>>);
impl PermissionReply {
    fn respond(mut self, response: Value) -> Result<(), String> {
        let response = serde_json::from_value(response)
            .map_err(|error| format!("Invalid permission response: {error}"))?;
        if let Some(reply) = self.0.take() {
            reply.respond(response).map_err(|error| error.to_string())?;
        }
        Ok(())
    }
}
impl Drop for PermissionReply {
    fn drop(&mut self) {
        if let Some(reply) = self.0.take() {
            let _ = reply.respond(RequestPermissionResponse::new(
                RequestPermissionOutcome::Cancelled,
            ));
        }
    }
}

enum Incoming {
    Update(Value),
    Permission(Value, PermissionReply),
    Response(u64, Result<Value, String>),
    Closed(String),
}

struct Cli {
    child: Option<ProcessScope>,
    ready: Option<oneshot::Receiver<ConnectionTo<Agent>>>,
    connection: Option<ConnectionTo<Agent>>,
    events: mpsc::UnboundedReceiver<Incoming>,
    outgoing: mpsc::UnboundedSender<Incoming>,
    shutdown: Option<oneshot::Sender<()>>,
    driver: tokio::task::JoinHandle<()>,
    next_id: u64,
    session: String,
    image: bool,
    backend: String,
    session_meta: Value,
    system_prompt: Option<String>,
    recovery_limit: usize,
    terminal_result: bool,
}

impl Drop for Cli {
    fn drop(&mut self) {
        self.driver.abort();
    }
}

impl Cli {
    fn transport(
        child: Option<ProcessScope>,
        input: impl AsyncWrite + Unpin + Send + 'static,
        output: impl AsyncRead + Unpin + Send + 'static,
    ) -> Self {
        let (outgoing, events) = mpsc::unbounded_channel();
        let notify = outgoing.clone();
        let permissions = outgoing.clone();
        let closed = outgoing.clone();
        let (connected, ready) = oneshot::channel();
        let (shutdown, stopped) = oneshot::channel();
        // Bound line allocation while the SDK handles all JSON-RPC envelopes and IDs.
        let reader = FramedRead::new(output, LinesCodec::new_with_max_length(FRAME_LIMIT))
            .map(|line| line.map_err(std::io::Error::other));
        let writer = futures::sink::unfold(input, |mut input, line: String| async move {
            input.write_all(line.as_bytes()).await?;
            input.write_all(b"\n").await?;
            input.flush().await?;
            Ok::<_, std::io::Error>(input)
        });
        let driver = tokio::spawn(async move {
            let result = Client
                .builder()
                .on_receive_notification(
                    async move |message: UntypedMessage, _cx| {
                        if message.method == "session/update" {
                            let _ = notify.send(Incoming::Update(message.params));
                        }
                        Ok(())
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .on_receive_request(
                    async move |request: RequestPermissionRequest, reply, _cx| {
                        let params = serde_json::to_value(request)?;
                        let _ = permissions
                            .send(Incoming::Permission(params, PermissionReply(Some(reply))));
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    async move |_request: UntypedMessage, reply, _cx| {
                        reply
                            .respond_with_error(agent_client_protocol::Error::method_not_found())?;
                        Ok(())
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .connect_with(Lines::new(writer, reader), async move |connection| {
                    let _ = connected.send(connection.clone());
                    tokio::select! {
                        _ = stopped => {},
                        _ = connection.incoming_closed() => {},
                    }
                    Ok(())
                })
                .await;
            let message = result.err().map_or_else(
                || "ACP agent closed its connection".into(),
                |error| error.to_string(),
            );
            let _ = closed.send(Incoming::Closed(message));
        });
        Self {
            child,
            ready: Some(ready),
            connection: None,
            events,
            outgoing,
            shutdown: Some(shutdown),
            driver,
            next_id: 0,
            session: String::new(),
            image: false,
            backend: "opencode".into(),
            session_meta: json!({}),
            system_prompt: None,
            recovery_limit: 5,
            terminal_result: false,
        }
    }

    async fn connection(&mut self) -> Result<ConnectionTo<Agent>, String> {
        if let Some(ready) = self.ready.take() {
            self.connection = Some(ready.await.map_err(|_| "ACP connection failed to start")?);
        }
        self.connection
            .clone()
            .ok_or_else(|| "ACP connection is unavailable".into())
    }

    async fn spawn(
        prefs: &auris_session::AgentPreferences,
        cwd: &Path,
        project: Option<&Path>,
        mcp: Option<&LiveMcp>,
        native: bool,
    ) -> Result<(Self, Value), String> {
        let launch = super::acp_launch::prepare(prefs, cwd, native, mcp, project).await?;
        let mut command = launch.command;
        command.current_dir(project.unwrap_or(cwd));
        let mut child = ProcessScope::spawn(command)
            .map_err(|error| format!("Cannot start {} ACP agent: {error}", prefs.backend))?;
        let input = child
            .child_mut()
            .stdin
            .take()
            .ok_or("ACP agent stdin was not available")?;
        let output = child
            .child_mut()
            .stdout
            .take()
            .ok_or("ACP agent stdout was not available")?;
        let mut cli = Self::transport(Some(child), input, output);
        cli.backend.clone_from(&prefs.backend);
        cli.recovery_limit = prefs.tool_failure_limit();
        cli.session_meta = launch.session_meta;
        cli.system_prompt = launch.system_prompt;
        let session = cli
            .initialize(
                project.unwrap_or(cwd),
                if launch.mcp_in_session { mcp } else { None },
            )
            .await?;
        if let Some(mcp) = mcp {
            tokio::time::timeout(STARTUP_TIMEOUT, mcp.wait_for_catalog())
                .await
                .map_err(|_| "ACP agent did not connect to the Auris live tool catalogue")?;
        }
        Ok((cli, session))
    }

    async fn request(
        &mut self,
        method: &str,
        params: Value,
        bridge: Option<&Bridge>,
    ) -> Result<Value, String> {
        self.terminal_result = false;
        let connection = self.connection().await?;
        self.next_id += 1;
        let id = self.next_id;
        let outgoing = self.outgoing.clone();
        let request = UntypedMessage::new(method, params).map_err(|error| error.to_string())?;
        // Ordered callbacks preserve updates before their prompt response, even on a fast peer.
        connection
            .prepare_request(request)
            .on_receiving_result(async move |result| {
                let _ = outgoing.send(Incoming::Response(
                    id,
                    result.map_err(|error| error.to_string()),
                ));
                Ok(())
            })
            .map_err(|error| error.to_string())?;
        let mut turn = Turn::default();
        loop {
            match self.events.recv().await.ok_or("ACP event stream ended")? {
                Incoming::Update(params) if params["sessionId"] == self.session => {
                    turn.update(&params["update"], bridge)?
                }
                Incoming::Update(_) => {}
                Incoming::Permission(params, reply) => {
                    let response = self.permission_response(&params, bridge, &turn).await;
                    reply.respond(response)?;
                }
                Incoming::Response(response_id, result) if response_id == id => {
                    self.terminal_result = true;
                    let mut result = result?;
                    if method == "session/prompt" {
                        if result["stopReason"] != "end_turn" {
                            return Err(format!(
                                "ACP turn did not complete: {}",
                                result["stopReason"].as_str().unwrap_or("unknown reason")
                            ));
                        }
                        result["answer"] = json!(turn.answer);
                        result["aurisToolResults"] = json!(turn.results);
                    }
                    return Ok(result);
                }
                Incoming::Response(_, _) => {}
                Incoming::Closed(error) => return Err(error),
            }
        }
    }

    async fn stop(&mut self) {
        if !self.session.is_empty()
            && let Ok(connection) = self.connection().await
            && let Ok(cancel) =
                UntypedMessage::new("session/cancel", json!({"sessionId":self.session}))
        {
            let _ = connection.send_notification(cancel);
        }
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(Duration::from_millis(500), &mut self.driver)
            .await
            .is_err()
        {
            self.driver.abort();
        }
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill().await;
        }
    }
}
#[derive(Default)]
struct Turn {
    answer: String,
    calls: HashMap<String, (String, bool, Value)>,
    results: Vec<(String, Value, bool)>,
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
                let bridge = bridge.ok_or("Approval reviewer attempted to use a tool")?;
                let id = update["toolCallId"]
                    .as_str()
                    .ok_or("ACP omitted a tool call ID")?;
                if update["sessionUpdate"] == "tool_call" {
                    let name = update["name"]
                        .as_str()
                        .or_else(|| update["title"].as_str())
                        .ok_or("ACP omitted a tool name")?;
                    // ACP titles are presentation text, not executable tool identities.
                    // Only the authenticated MCP namespace denotes an Auris tool.
                    // Preserve the desktop's native operation names: review evidence and
                    // permission traces use these identities, never an agent's display title.
                    let name = auris_tool_name(name)
                        .or_else(|| native_operation(update))
                        .unwrap_or(name);
                    self.calls
                        .insert(id.to_owned(), (name.to_owned(), false, json!({})));
                }
                if let Some((name, emitted, args)) = self.calls.get_mut(id) {
                    let input = if ToolHost::contains(name)
                        && update["rawInput"]["server"] == "auris_live"
                    {
                        &update["rawInput"]["arguments"]
                    } else {
                        &update["rawInput"]
                    };
                    if let Some(input) = input.as_object() {
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
                        self.results.push((
                            name.clone(),
                            args.clone(),
                            update["status"] == "completed",
                        ));
                        self.calls.remove(id);
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn auris_tool_name(name: &str) -> Option<&str> {
    [
        "mcp.auris_live.",
        "mcp__auris_live__",
        "mcp__auris_live_",
        "auris_live_",
        "auris_live/",
        "auris_live.",
    ]
    .iter()
    .find_map(|prefix| {
        name.strip_prefix(prefix)
            .filter(|name| ToolHost::contains(name))
    })
}

fn native_operation(tool: &Value) -> Option<&'static str> {
    match tool["kind"].as_str().unwrap_or_default() {
        "execute" => Some("cli_command"),
        "edit" | "delete" | "move" => Some("cli_file_change"),
        "read" | "search" => Some("cli_file_read"),
        // Directory access still leaves the actual command/edit subject to its own gate.
        _ if tool["title"] == "external_directory" => Some("cli_file_read"),
        _ => None,
    }
}

fn model_config(session: &Value) -> Option<&Value> {
    session["configOptions"]
        .as_array()?
        .iter()
        .find(|config| config["category"] == "model" || config["id"] == "model")
}

fn option_values(options: &Value) -> Vec<(&str, &str)> {
    let mut result = Vec::new();
    if let Some(options) = options.as_array() {
        for option in options {
            if let Some(value) = option["value"].as_str().filter(|id| !id.is_empty()) {
                result.push((value, option["name"].as_str().unwrap_or(value)));
            } else {
                result.extend(option_values(&option["options"]));
            }
        }
    }
    result
}

fn models(session: &Value) -> Result<String, String> {
    let efforts: Vec<_> = session["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|config| config["category"] == "thought_level")
        .map(|config| {
            option_values(&config["options"])
                .into_iter()
                .filter_map(|(value, _)| {
                    auris_session::ReasoningEffort::named(value).map(|effort| effort.as_str())
                })
                .collect()
        })
        .unwrap_or_default();
    let result: Vec<_> = if let Some(config) = model_config(session) {
        option_values(&config["options"])
            .into_iter()
            .map(|(id, name)| json!({"name":id,"display_name":name,"efforts":efforts}))
            .collect()
    } else {
        session["models"]["availableModels"].as_array().into_iter().flatten()
            .filter_map(|entry| entry["modelId"].as_str().filter(|id| !id.is_empty())
                .map(|id| json!({"name":id,"display_name":entry["name"].as_str().unwrap_or(id),"efforts":efforts}))).collect()
    };
    if result.is_empty() {
        return Err(
            "ACP returned no models. Check the agent's login and provider configuration".into(),
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
                return Err("This ACP CLI does not support image attachments".into());
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
    .map_err(|_| "ACP startup timed out".to_string())??;
    let result = models(&session);
    cli.stop().await;
    result
}

pub(crate) async fn review(
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
        result = tokio::time::timeout(STARTUP_TIMEOUT, Cli::spawn(prefs, cwd.path(), project, Some(&mcp), true)) => result.map_err(|_| "ACP startup timed out".to_string())??,
    };
    let result = tokio::select! {
        biased;
        _ = &mut cancelled => Ok(()),
        _ = mcp.cancelled() => Err("ACP cancelled a pending live tool request. Inspect the document before continuing".into()),
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
            bridge.emit(json!({"event":"compacted","ok":false,"manual":true,"message":"Manual compaction is not available for the ACP backend yet"}));
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
        // A leading slash belongs to the Auris request, never to ACP's
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
        match answer {
            Ok(text) => bridge.emit(json!({"event":"answer","text":text})),
            Err(message) if cli.terminal_result => {
                bridge.emit(json!({"event":"error","message":message}))
            }
            Err(message) => return Err(message),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;

impl Cli {
    async fn initialize(&mut self, root: &Path, mcp: Option<&LiveMcp>) -> Result<Value, String> {
        let initialized = self.request("initialize", json!({"protocolVersion":1,
            "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false},
            "clientInfo":{"name":"auris-studio","version":env!("CARGO_PKG_VERSION")}}), None).await?;
        if initialized["protocolVersion"] != 1 {
            return Err("ACP requires a compatible ACP v1 CLI".into());
        }
        if mcp.is_some() && initialized["agentCapabilities"]["mcpCapabilities"]["http"] != true {
            return Err("ACP does not support the HTTP MCP transport required by Auris".into());
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
        let mut params = json!({"cwd":root,"mcpServers":servers,"_meta":self.session_meta});
        if let Some(prompt) = &self.system_prompt {
            let capability =
                &initialized["agentCapabilities"]["sessionCapabilities"]["systemPrompt"];
            if !capability.is_object() {
                return Err(
                    "Update the ACP adapter: it must support client system instructions".into(),
                );
            }
            params["systemPrompt"] = json!({"mode":if capability["mode"]["override"].is_object() {"override"} else {"append"},
                "content":[{"type":"text","text":prompt}]});
        }
        let session = self.request("session/new", params, None).await?;
        self.session = session["sessionId"]
            .as_str()
            .filter(|id| !id.is_empty())
            .ok_or("ACP did not return an ACP session ID")?
            .to_owned();
        // Use the configured primary agent, independently of the CLI's default mode.
        let mode_config = session["configOptions"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|config| {
                config["category"] == "mode"
                    && option_values(&config["options"])
                        .iter()
                        .any(|(value, _)| *value == "auris")
            });
        let auris_mode = mode_config.is_some()
            || session["modes"]["availableModes"]
                .as_array()
                .is_some_and(|modes| modes.iter().any(|mode| mode["id"] == "auris"));
        if self.backend == "opencode" && !auris_mode {
            return Err("OpenCode did not load the isolated Auris agent configuration".into());
        }
        if let Some(config) = mode_config {
            self.request(
                "session/set_config_option",
                json!({"sessionId":self.session,"configId":config["id"],"value":"auris"}),
                None,
            )
            .await?;
        } else if auris_mode {
            self.request(
                "session/set_mode",
                json!({"sessionId":self.session,"modeId":"auris"}),
                None,
            )
            .await?;
        }
        Ok(session)
    }

    async fn permission_response(
        &mut self,
        params: &Value,
        bridge: Option<&Bridge>,
        turn: &Turn,
    ) -> Value {
        let tool = &params["toolCall"];
        let operation = native_operation(tool);
        let mut allowed = false;
        let live_tool = tool["name"]
            .as_str()
            .or_else(|| tool["title"].as_str())
            .and_then(auris_tool_name)
            .or_else(|| {
                tool["toolCallId"]
                    .as_str()
                    .and_then(|id| turn.calls.get(id))
                    .map(|(name, _, _)| name.as_str())
                    .filter(|name| ToolHost::contains(name))
            });
        // The live server applies the authoritative document permission gate at execution.
        if params["sessionId"] == self.session && bridge.is_some() && live_tool.is_some() {
            allowed = true;
        }
        if params["sessionId"] == self.session
            && live_tool.is_none()
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
                Some(name) => crate::permissions::call_origin(call_id, name, "acp_approval"),
                None => crate::permissions::PermissionOrigin::Independent {
                    source: "acp_approval_without_tool_identity".into(),
                },
            };
            allowed = crate::permissions::with_origin(origin, crate::permissions::authorize(Some(bridge), operation,
                &json!({"backend":self.backend,"tool":tool["title"],"input":tool["rawInput"],"request":tool,"within_project":false}).to_string())).await.is_ok();
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
        json!({"outcome":outcome})
    }
    async fn select_model(
        &mut self,
        prefs: &auris_session::AgentPreferences,
        session: &Value,
    ) -> Result<(), String> {
        let mut settings = session.clone();
        if !prefs.model.is_empty() {
            let result = if let Some(config) = model_config(session) {
                self.request(
                    "session/set_config_option",
                    json!({"sessionId":self.session,"configId":config["id"],"value":prefs.model}),
                    None,
                )
                .await?
            } else if session["models"]["availableModels"].is_array() {
                self.request(
                    "session/set_model",
                    json!({"sessionId":self.session,"modelId":prefs.model}),
                    None,
                )
                .await?
            } else {
                return Err("ACP did not advertise model selection".into());
            };
            if result["configOptions"].is_array() {
                settings["configOptions"] = result["configOptions"].clone();
            }
        }
        if prefs.effort != auris_session::ReasoningEffort::Default {
            let effort = match (self.backend.as_str(), prefs.effort) {
                (
                    "claude",
                    auris_session::ReasoningEffort::None | auris_session::ReasoningEffort::Minimal,
                ) => "low",
                _ => prefs.effort.as_str(),
            };
            if let Some(config) = settings["configOptions"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|config| config["category"] == "thought_level")
            {
                if !option_values(&config["options"])
                    .iter()
                    .any(|(value, _)| *value == effort)
                {
                    return Err(format!(
                        "ACP model does not support reasoning effort '{effort}'"
                    ));
                }
                self.request(
                    "session/set_config_option",
                    json!({"sessionId":self.session,"configId":config["id"],"value":effort}),
                    None,
                )
                .await?;
            } else if self.backend != "opencode" {
                return Err("ACP adapter did not advertise reasoning effort selection".into());
            }
        }
        Ok(())
    }

    async fn prompt(&mut self, content: Value, bridge: Option<&Bridge>) -> Result<String, String> {
        let mut content = content;
        let mut recovery = recovery::Recovery::new(self.recovery_limit);
        loop {
            recovery.begin_turn();
            let answer = self
                .request(
                    "session/prompt",
                    json!({"sessionId":self.session,"prompt":content}),
                    bridge,
                )
                .await?;
            if self.backend == "claude"
                && let Some(bridge) = bridge
            {
                let results: Vec<(String, Value, bool)> =
                    serde_json::from_value(answer["aurisToolResults"].clone())
                        .map_err(|error| error.to_string())?;
                for (name, args, ok) in results {
                    recovery.observe(&name, &args.to_string(), ok, ToolHost::contains(&name));
                }
                if let Some(prompt) = recovery.continuation(bridge)? {
                    content = json!([{"type":"text","text":prompt}]);
                    continue;
                }
            }
            return Ok(answer["answer"].as_str().unwrap_or_default().to_owned());
        }
    }
}
