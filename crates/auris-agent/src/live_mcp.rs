//! Per-conversation MCP endpoint for the open document, never the saved-file catalog.

use crate::host::ToolHost;
use axum::{
    Router,
    body::Body,
    extract::Request,
    http::StatusCode,
    middleware::{self, Next},
    response::Response,
};
use rmcp::{
    ServerHandler,
    model::*,
    service::RequestContext,
    transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    },
};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use tokio_util::sync::CancellationToken;

#[cfg(test)]
pub(crate) async fn transcription_test_call(
    client: &reqwest::Client,
    url: &str,
    token: &str,
    session: &str,
    attachment: &serde_json::Value,
    name: &str,
) -> serde_json::Value {
    use serde_json::{Value, json};
    let request = |id, name: &str, args: Value| {
        client.post(url).bearer_auth(token).header("Accept","application/json, text/event-stream").header("mcp-session-id",session).json(&json!({"jsonrpc":"2.0","id":id,"method":"tools/call","params":{"name":name,"arguments":args}}))
    };
    let parse = |text: &str| {
        serde_json::from_str::<Value>(text)
            .ok()
            .or_else(|| {
                text.lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .find_map(|line| serde_json::from_str(line).ok())
            })
            .expect("MCP JSON or SSE")
    };
    let recognized = request(
        2,
        "transcribe_audio",
        json!({"attachment":attachment,"backend":{"kind":"basic_pitch"}}),
    )
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
    let recognized = parse(&recognized);
    assert_ne!(recognized["result"]["isError"], true, "{recognized}");
    let draft: Value =
        serde_json::from_str(recognized["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let imported = request(
        3,
        "import_transcription",
        json!({"draft_id":draft["draft_id"],"name":name}),
    )
    .send()
    .await
    .unwrap()
    .text()
    .await
    .unwrap();
    parse(&imported)
}

#[derive(Clone)]
struct LiveTools {
    host: ToolHost,
    active: Arc<AtomicBool>,
    failure: CancellationToken,
    catalog_ready: CancellationToken,
}

impl ServerHandler for LiveTools {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let tools = ToolHost::definitions()
            .into_iter()
            .map(|definition| {
                Tool::new(
                    definition.name,
                    definition.description,
                    definition
                        .parameters
                        .as_object()
                        .cloned()
                        .unwrap_or_default(),
                )
            })
            .collect();
        self.catalog_ready.cancel();
        Ok(ListToolsResult {
            tools,
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let result = if self.active.load(Ordering::Acquire) {
            tokio::select! {
                biased;
                _ = self.failure.cancelled() => Err("The live tool connection closed".into()),
                _ = context.ct.cancelled() => {
                    // End the worker as well: a late approval must not be read as a new prompt.
                    self.failure.cancel();
                    Err("The live tool request was cancelled".into())
                }
                reply = self.host.call(&request.name, serde_json::Value::Object(request.arguments.unwrap_or_default())) => reply,
            }
        } else {
            Err("No active agent turn".into())
        };
        let result = match result {
            Ok(reply) if reply["ok"] == true => {
                if let Some(report) = reply.get("inspection") {
                    match serde_json::from_value(report.clone())
                        .map_err(|e| e.to_string())
                        .and_then(|report| crate::toolbox::audio_inspection::present(&report))
                    {
                        Ok(presentation) => CallToolResult::success(vec![
                            ContentBlock::text(presentation.text),
                            ContentBlock::image(presentation.png, "image/png"),
                        ]),
                        Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
                    }
                } else {
                    CallToolResult::success(vec![ContentBlock::text(
                        reply["text"].as_str().unwrap_or_default(),
                    )])
                }
            }
            Ok(reply) => CallToolResult::error(vec![ContentBlock::text(
                reply["text"].as_str().unwrap_or("The live edit failed"),
            )]),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        };
        Ok(result.into())
    }
}

/// Owns a loopback listener and revokes it when its conversation ends.
pub(crate) struct LiveMcp {
    pub(crate) url: String,
    pub(crate) token: String,
    active: Arc<AtomicBool>,
    failure: CancellationToken,
    catalog_ready: CancellationToken,
    cancellation: CancellationToken,
    task: tokio::task::JoinHandle<()>,
}

impl LiveMcp {
    pub(crate) async fn start(host: ToolHost) -> Result<Self, String> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| e.to_string())?;
        let url = format!(
            "http://{}/mcp",
            listener.local_addr().map_err(|e| e.to_string())?
        );
        let token = uuid::Uuid::new_v4().simple().to_string();
        let authorization = format!("Bearer {token}");
        let active = Arc::new(AtomicBool::new(false));
        let failure = CancellationToken::new();
        let catalog_ready = CancellationToken::new();
        let handler = LiveTools {
            host,
            active: active.clone(),
            failure: failure.clone(),
            catalog_ready: catalog_ready.clone(),
        };
        let cancellation = CancellationToken::new();
        let service: StreamableHttpService<LiveTools, LocalSessionManager> =
            StreamableHttpService::new(
                move || Ok(handler.clone()),
                Default::default(),
                StreamableHttpServerConfig::default()
                    .with_json_response(true)
                    .with_cancellation_token(cancellation.clone()),
            );
        let router = Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn(move |request: Request, next: Next| {
                let authorization = authorization.clone();
                async move {
                    if request.headers().contains_key("origin")
                        || request
                            .headers()
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            != Some(authorization.as_str())
                    {
                        return Response::builder()
                            .status(StatusCode::UNAUTHORIZED)
                            .body(Body::empty())
                            .unwrap_or_default();
                    }
                    next.run(request).await
                }
            }));
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, router)
                .with_graceful_shutdown(shutdown.cancelled_owned())
                .await;
        });
        Ok(Self {
            url,
            token,
            active,
            failure,
            catalog_ready,
            cancellation,
            task,
        })
    }

    /// Wait until the authenticated client has requested the live tool catalogue.
    pub(crate) async fn wait_for_catalog(&self) {
        self.catalog_ready.cancelled().await;
    }

    pub(crate) fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Release);
    }

    pub(crate) async fn cancelled(&self) {
        self.failure.cancelled().await;
    }

    pub(crate) fn abort_calls(&self) {
        self.set_active(false);
        self.failure.cancel();
    }
}

impl Drop for LiveMcp {
    fn drop(&mut self) {
        self.abort_calls();
        self.cancellation.cancel();
        self.task.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::time::Duration;

    fn rpc_response(body: &str) -> Value {
        serde_json::from_str(body)
            .ok()
            .or_else(|| {
                body.lines()
                    .filter_map(|line| line.strip_prefix("data: "))
                    .find_map(|data| serde_json::from_str(data).ok())
            })
            .expect("a JSON response or an SSE data message")
    }

    #[tokio::test]
    async fn phrase_generation_is_advertised_and_permission_checked_on_the_cli_mcp_endpoint() {
        tokio::time::timeout(Duration::from_secs(10), async {
            for allowed in [false, true] {
                let (bridge, commands, events) = crate::worker::test_bridge();
                let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
                mcp.set_active(true);
                let client = reqwest::Client::new();
                let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token)
                    .header("Accept", "application/json, text/event-stream").json(&value);
                let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"phrase-fixture","version":"1"}}})).send().await.unwrap();
                let session_id = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
                request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session_id).send().await.unwrap();
                let response = request(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})).header("mcp-session-id", &session_id).send().await.unwrap();
                assert!(response.status().is_success());
                let body = response.text().await.unwrap();
                tokio::time::timeout(Duration::from_secs(1), mcp.wait_for_catalog()).await.unwrap();
                let tools = rpc_response(&body);
                let tool = tools["result"]["tools"].as_array().unwrap().iter().find(|tool| tool["name"] == "generate_phrase").unwrap();
                assert_eq!(tool["inputSchema"]["properties"]["bars"]["maximum"], 1024);
                let mut song = auris_session::Session::new(auris_session::SessionOptions::headless().with_balance(false)).unwrap();
                let track = song.add_default_instrument_track("Lead").unwrap();
                song.agent_command(serde_json::from_value(json!({"action":"edit_harmony","start_bar":1,"bars":4,"chords":"| I | IV | V | I |"})).unwrap()).unwrap();
                let before = song.project().clone();
                let call = async {
                    let response = request(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"generate_phrase","arguments":{"track":track.0,"part":"lead","start_bar":1,"bars":4,"seed":42}}})).header("mcp-session-id", &session_id).send().await.unwrap();
                    assert!(response.status().is_success());
                    rpc_response(&response.text().await.unwrap())
                };
                let owner = async {
                    loop {
                        let event = loop {
                            match events.try_recv() {
                                Ok(event) => break event,
                                Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                                Err(error) => panic!("{error}"),
                            }
                        };
                        if event["event"] == "permission" {
                            let operation = auris_session::agent_policy::Operation::parse(event["tool"].as_str().unwrap(), &event["args"]).unwrap();
                            assert_eq!(operation.name, "edit_project.generate_phrase");
                            let policy = auris_session::agent_policy::Policy { mode: if allowed { auris_session::agent_policy::Mode::Edit } else { auris_session::agent_policy::Mode::ReadOnly }, ..Default::default() };
                            assert_eq!(policy.decide(&operation) == auris_session::agent_policy::Decision::Allow, allowed);
                            commands.send(json!({"event":"permission_result","id":event["id"],"ok":allowed,"reason":"fixture policy"}).to_string()).unwrap();
                            if !allowed { break; }
                        } else if event["event"] == "edit" {
                            assert!(allowed);
                            let text = song.agent_command(serde_json::from_value(event["command"].clone()).unwrap()).unwrap();
                            let result: Value = serde_json::from_str(&text).unwrap();
                            assert!(result["notes"].as_u64().unwrap() > 0);
                            assert!(song.path().is_none());
                            commands.send(json!({"event":"edit_result","ok":true,"text":text}).to_string()).unwrap();
                            song.undo();
                            break;
                        }
                    }
                    assert_eq!(song.project(), &before);
                };
                let (result, ()) = tokio::join!(call, owner);
                assert_eq!(result["result"]["isError"].as_bool().unwrap_or(false), !allowed, "{result}");
                assert!(events.try_recv().is_err());
            }
        }).await.unwrap();
    }

    #[tokio::test]
    async fn cancelled_mcp_requests_cannot_execute_after_a_late_approval() {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
            mcp.set_active(true);
            let client = reqwest::Client::new();
            let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token)
                .header("Accept", "application/json, text/event-stream").json(&value);
            let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"cancellation-fixture","version":"1"}}})).send().await.unwrap();
            let session = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
            request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session).send().await.unwrap();
            let call = request(json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"add_track","arguments":{"name":"Late","kind":"instrument"}}})).header("mcp-session-id", &session);
            let running = tokio::spawn(async move { call.send().await.unwrap().text().await.unwrap() });
            let permission = loop {
                if let Ok(event) = events.try_recv() { break event; }
                tokio::task::yield_now().await;
            };
            assert_eq!(permission["event"], "permission");
            request(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":9,"reason":"fixture timeout"}})).header("mcp-session-id", &session).send().await.unwrap();
            mcp.cancelled().await;
            // MCP cancellation may close the SSE response without a result message.
            // Waiting for it also waits for the handler to drop the pending host call.
            let _ = running.await.unwrap();
            commands.send(json!({"event":"permission_result","id":permission["id"],"ok":true}).to_string()).unwrap();
            assert!(events.try_recv().is_err(), "A cancelled call requested a late edit");
        }).await.unwrap();
    }
}
