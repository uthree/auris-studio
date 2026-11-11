//! `auris-mcp` — the Model Context Protocol frontend.
//!
//! The third frontend, and the first with no person at it: an MCP client — a language model's
//! harness — connects over stdio and drives the same session the desktop application and the
//! command line do. Everything the tools *are* — names, descriptions, argument schemas and the
//! work behind them — lives in [`auris_toolbox`], shared with `auris-agent` so the two doors a
//! model comes through can never drift apart; this crate is the stdio door and nothing else.
//!
//! Registration and dispatch use the toolbox runtime registry.
//!
//! What remains here is the protocol binding, and its two decisions:
//!
//! * **Errors are tool answers.** A mistake a model can read and fix (a wrong path, a rejected
//!   spec) comes back as a result with `is_error` set, not as a protocol error — MCP reserves
//!   those for the server itself breaking.
//! * **Blocking work leaves the runtime.** Every tool that opens a session runs inside
//!   `spawn_blocking`, both because the work is honest blocking DSP and because a session
//!   created and dropped inside one closure never has to be `Send`.

#![warn(missing_docs)]

use auris_toolbox as toolbox;

use rmcp::model::{
    CallToolResult, ContentBlock, ErrorData, Implementation, ServerCapabilities, ServerConfig,
};
use rmcp::{ServerHandler, ServiceExt};

tokio::task_local! {
    /// Cancellation state for the request currently executing on this async task.
    static REQUEST_CANCELLATION: std::sync::Arc<toolbox::Cancellation>;
}

/// Saved projects and rendered audio stay on disk.
#[derive(Clone, Debug, Default)]
struct AurisMcp {
    groups: toolbox::ToolGroups,
}

impl ServerHandler for AurisMcp {
    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        let tools = toolbox::tool_catalog()
            .into_iter()
            .filter(|t| self.groups.allows(&t.name))
            .map(|mut t| {
                toolbox::compact_parameters(&mut t.parameters);
                rmcp::model::Tool::new(
                    t.name,
                    toolbox::concise_description(&t.description),
                    t.parameters.as_object().expect("object schema").clone(),
                )
            })
            .collect();
        Ok(rmcp::model::ListToolsResult {
            tools,
            ..Default::default()
        })
    }
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, ErrorData> {
        let name = request.name.to_string();
        if !self.groups.allows(&name) {
            return Ok(CallToolResult::error(vec![ContentBlock::text(format!("Tool {name} is disabled. Enable group '{}' in AURIS_MCP_TOOL_GROUPS and restart the server.", toolbox::tool_group(&name)))]).into());
        }
        let known = toolbox::tool_catalog().iter().any(|tool| tool.name == name);
        let arguments = serde_json::Value::Object(request.arguments.clone().unwrap_or_default());
        let protocol_probe = context.ct.clone();
        let cancellation = std::sync::Arc::new(toolbox::Cancellation::with_probe(move || {
            protocol_probe.is_cancelled()
        }));
        let cancellation_request = std::sync::Arc::clone(&cancellation);
        let protocol_token = context.ct.clone();
        // A dropped JoinHandle leaves its task running. That matters when rmcp drops this handler
        // future on cancellation: the blocking worker still receives the signal and stops before
        // it can cross the toolbox's commit gate.
        let cancellation_watcher = tokio::spawn(async move {
            protocol_token.cancelled().await;
            cancellation_request.cancel();
        });
        let groups = self.groups.clone();
        let call_name = name.clone();
        let call_args = arguments.clone();
        let result = REQUEST_CANCELLATION
            .scope(
                cancellation,
                blocking_tool(Answer::of(&name), move || {
                    toolbox::runtime::call_saved(&groups, &call_name, call_args)
                }),
            )
            .await
            .map(rmcp::model::CallToolResponse::Complete);
        cancellation_watcher.abort();
        recover_argument_error(result, &name, known, Some(&arguments))
    }
    fn get_info(&self) -> ServerConfig {
        // Field by field because the type is `non_exhaustive`, which rules the literal out.
        // Named explicitly rather than via `Implementation::from_build_env`, whose `env!` was
        // expanded when *rmcp* was compiled — a server introducing itself as "rmcp 3.1.4".
        let mut info = ServerConfig::default();
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.server_info = Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        info.instructions = Some(self.groups.instructions());
        info
    }
}

/// Invalid arguments are recoverable model mistakes; unknown tools and server errors retain
/// their protocol error codes.
fn recover_argument_error(
    result: Result<rmcp::model::CallToolResponse, ErrorData>,
    name: &str,
    known: bool,
    arguments: Option<&serde_json::Value>,
) -> Result<rmcp::model::CallToolResponse, ErrorData> {
    let guidance = || {
        let hint = arguments
            .and_then(|args| toolbox::argument_error_hint(name, args))
            .map(|hint| format!("{hint}. "))
            .unwrap_or_default();
        format!(
            "{hint}Call tool_help with name '{name}' for exact fields and examples, then correct the arguments."
        )
    };
    match result {
        Err(error) if known && error.code == rmcp::model::ErrorCode::INVALID_PARAMS => {
            Ok(CallToolResult::error(vec![ContentBlock::text(format!(
                "Invalid arguments for {name}: {}. {}",
                error.message,
                guidance()
            ))])
            .into())
        }
        Ok(rmcp::model::CallToolResponse::Complete(mut result))
            if known
                && result.is_error == Some(true)
                && result
                    .content
                    .iter()
                    .filter_map(|content| content.as_text())
                    .any(|content| content.text.starts_with("Invalid arguments")) =>
        {
            // Argument failures from the shared registry are ordinary recoverable tool results.
            result.content.push(ContentBlock::text(guidance()));
            Ok(result.into())
        }
        other => other,
    }
}

/// Wraps work that touches a session or the filesystem, off the async runtime.
///
/// Everything behind these tools is honest blocking work — opening a session parses SoundFont
/// files, a render is minutes of DSP — and tokio's worker threads are for neither. `Ok` and
/// `Err` both become *results* here: an error a model can read and fix is a tool answer, not a
/// protocol failure, which MCP reserves for the server itself breaking — the one thing left
/// for the outer `Result`.
async fn blocking(
    work: impl FnOnce() -> Result<String, String> + Send + 'static,
) -> Result<CallToolResult, ErrorData> {
    finished(run_blocking(work).await?)
}

/// What a tool's text answer becomes on the wire.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Answer {
    /// The text itself.
    Text,
    /// `inspect_audio`: measurements and a PNG.
    Inspection,
    /// `listen`: the report and each clip as audio content, for a client whose model hears.
    Audition,
}

impl Answer {
    fn of(tool: &str) -> Self {
        match tool {
            "inspect_audio" => Self::Inspection,
            "listen" => Self::Audition,
            _ => Self::Text,
        }
    }
}

/// The image presenter is shared with the live host; transport only supplies MCP content blocks.
async fn blocking_tool(
    answer: Answer,
    work: impl FnOnce() -> Result<String, String> + Send + 'static,
) -> Result<CallToolResult, ErrorData> {
    if answer == Answer::Text {
        return blocking(work).await;
    }
    let text = match run_blocking(work).await? {
        Err(error) => return finished(Err(error)),
        Ok(text) => text,
    };
    let blocks = if answer == Answer::Inspection {
        serde_json::from_str(&text)
            .map_err(|e| e.to_string())
            .and_then(|report| toolbox::audio_inspection::present(&report))
            .map(|report| {
                vec![
                    ContentBlock::text(report.text),
                    ContentBlock::image(report.png, "image/png"),
                ]
            })
    } else {
        serde_json::from_str::<auris_session::audio_audition::Audition>(&text)
            .map_err(|e| e.to_string())
            .map(|audition| {
                let mut blocks = vec![ContentBlock::text(audition.text())];
                for clip in &audition.clips {
                    blocks.push(ContentBlock::text(audition.caption(clip)));
                    blocks.push(ContentBlock::audio(clip.wav_base64.clone(), "audio/wav"));
                }
                blocks
            })
    };
    match blocks {
        Ok(blocks) => Ok(CallToolResult::success(blocks)),
        Err(error) => finished(Err(error)),
    }
}

/// Runs one synchronous toolbox call with the current MCP cancellation state installed on its
/// pooled worker thread.
async fn run_blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<Result<T, String>, ErrorData> {
    let cancellation = REQUEST_CANCELLATION
        .try_with(std::sync::Arc::clone)
        .unwrap_or_else(|_| std::sync::Arc::new(toolbox::Cancellation::new()));
    let observed = std::sync::Arc::clone(&cancellation);
    tokio::task::spawn_blocking(move || {
        toolbox::with_cancellation(cancellation, || {
            if observed.is_cancelled() {
                return Err("the tool request was cancelled before starting".to_string());
            }
            let outcome = work();
            if observed.is_cancelled() {
                return Err("the tool request was cancelled before making durable changes".into());
            }
            outcome
        })
    })
    .await
    .map_err(|error| ErrorData::internal_error(error.to_string(), None))
}

/// Turns a tool's verdict into the result the protocol carries.
fn finished(outcome: Result<String, String>) -> Result<CallToolResult, ErrorData> {
    Ok(match outcome {
        Ok(text) => CallToolResult::success(vec![ContentBlock::text(text)]),
        Err(text) => CallToolResult::error(vec![ContentBlock::text(text)]),
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(code) = auris_session::handle_drum_probe_worker() {
        std::process::exit(code);
    }
    if let Some(code) = auris_session::handle_plugin_discovery_worker() {
        std::process::exit(code);
    }
    // Stderr, and only stderr: stdout is the protocol channel, and one stray line on it is a
    // broken connection.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    // Like the CLI: nothing here reads the configuration, but this may still be the frontend
    // that runs first on a machine, and an installation predating the move to
    // `~/.config/auris-studio` only has its settings carried across by whichever one does.

    tokio::runtime::Runtime::new()?.block_on(async {
        let groups = match std::env::var("AURIS_MCP_TOOL_GROUPS") {
            Ok(v) => toolbox::ToolGroups::parse(&v)?,
            Err(std::env::VarError::NotPresent) => toolbox::ToolGroups::default(),
            Err(e) => return Err(e.into()),
        };
        let service = AurisMcp { groups }.serve(rmcp::transport::stdio()).await?;
        service.waiting().await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shared_saved_inspection_is_delivered_as_measurements_and_a_png() {
        use auris_session::{Session, SessionOptions, prelude::*};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Inspect.auris");
        {
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let track = session.add_default_instrument_track("Lead").unwrap();
            let clip = session
                .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(3840))
                .unwrap();
            session
                .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks(997))])
                .unwrap();
            session.save(&path).unwrap();
        }
        let original = std::fs::read(&path).unwrap();
        let project = path.clone();
        let result = blocking_tool(Answer::Inspection, move || {
            toolbox::runtime::call_saved(
                &toolbox::ToolGroups::default(),
                "inspect_audio",
                serde_json::json!({"project":project,"start_bar":1,"bars":1}),
            )
        })
        .await
        .unwrap();
        let content = serde_json::to_value(result).unwrap();
        assert_ne!(content["isError"], true);
        assert_eq!(content["content"][0]["type"], "text");
        assert_eq!(content["content"][1]["type"], "image");
        assert_eq!(content["content"][1]["mimeType"], "image/png");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[tokio::test]
    async fn saved_listening_is_delivered_as_a_report_and_wav_audio() {
        use auris_session::{Session, SessionOptions, prelude::*};
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("Listen.auris");
        {
            let mut session = Session::new(SessionOptions::headless()).unwrap();
            let track = session.add_default_instrument_track("Lead").unwrap();
            let clip = session
                .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks(3840))
                .unwrap();
            session
                .replace_notes(clip, vec![Note::new(60, Ticks::ZERO, Ticks(997))])
                .unwrap();
            session.save(&path).unwrap();
        }
        let original = std::fs::read(&path).unwrap();
        let project = path.clone();
        let result = blocking_tool(Answer::of("listen"), move || {
            toolbox::runtime::call_saved(
                &toolbox::ToolGroups::default(),
                "listen",
                serde_json::json!({"project":project,"start_bar":1,"bars":1}),
            )
        })
        .await
        .unwrap();
        let content = serde_json::to_value(result).unwrap();
        assert_ne!(content["isError"], true, "{content}");
        let report: serde_json::Value =
            serde_json::from_str(content["content"][0]["text"].as_str().unwrap()).unwrap();
        assert!(report["audition_id"].is_string());
        assert!(
            content["content"][1]["text"]
                .as_str()
                .unwrap()
                .contains("(excerpt)")
        );
        assert_eq!(content["content"][2]["type"], "audio");
        assert_eq!(content["content"][2]["mimeType"], "audio/wav");
        let wav = auris_session::audio_audition::AuditionClip {
            label: "excerpt".into(),
            wav_base64: content["content"][2]["data"].as_str().unwrap().into(),
        }
        .wav()
        .unwrap();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(std::fs::read(&path).unwrap(), original);
    }

    #[tokio::test]
    async fn manual_catalog_is_compact_and_disabled_tools_are_not_callable() {
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp {
                groups: toolbox::ToolGroups::parse("manual").unwrap(),
            }
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let listed = client.list_tools(None).await.unwrap();
        assert!(listed.tools.iter().any(|t| t.name == "setup_tracks"));
        assert!(
            !listed
                .tools
                .iter()
                .any(|t| t.name == "transcribe_audio" || t.name == "automation")
        );
        let full = toolbox::tool_catalog();
        for actual in &listed.tools {
            let original = full.iter().find(|t| t.name == actual.name).unwrap();
            let mut compact = original.parameters.clone();
            toolbox::compact_parameters(&mut compact);
            assert_eq!(actual.input_schema.as_ref(), compact.as_object().unwrap());
        }
        let denied = client
            .call_tool(rmcp::model::CallToolRequestParams::new("automation"))
            .await
            .unwrap();
        assert_eq!(denied.is_error, Some(true));
        let help = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("tool_help").with_arguments(
                    serde_json::json!({"name":"edit_notes"})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(help.is_error, Some(true));
        let payload: serde_json::Value =
            serde_json::from_str(&help.content[0].as_text().unwrap().text).unwrap();
        let mut expected = full
            .iter()
            .find(|t| t.name == "edit_notes")
            .unwrap()
            .parameters
            .clone();
        toolbox::compact_parameters(&mut expected);
        let mut help_validation = payload["parameters"].clone();
        toolbox::compact_parameters(&mut help_validation);
        assert_eq!(help_validation, expected);
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn preview_returns_a_local_wav_path_without_binary_content() {
        use auris_session::prelude::{Note, Ticks};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Preview.auris");
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Lead").unwrap();
        let clip = session
            .add_midi_clip(track, "Phrase", Ticks::ZERO, Ticks::QUARTER * 4)
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::QUARTER))
            .unwrap();
        session.save(&path).unwrap();
        drop(session);
        assert!(
            AurisMcp::default()
                .get_info()
                .capabilities
                .resources
                .is_none()
        );
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp::default()
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let result = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("preview").with_arguments(
                    serde_json::json!({"project":path,"range":{"start_bar":1,"bars":1}})
                        .as_object()
                        .unwrap()
                        .clone(),
                ),
            )
            .await
            .unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert_eq!(result.content.len(), 1);
        let text = &result.content[0].as_text().unwrap().text;
        let wav = std::fs::read_dir(root.path().join(".auris-previews"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(wav.is_absolute());
        let metadata: serde_json::Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            std::path::Path::new(metadata["files"][0]["path"].as_str().unwrap()),
            wav
        );
        assert_eq!(metadata["files"][0]["encoded"]["sample_rate"], 24000);
        assert!(text.len() < 2048);
        let bytes = std::fs::read(wav).unwrap();
        assert!(bytes.starts_with(b"RIFF"));
        assert_eq!(&bytes[8..12], b"WAVE");
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn graph_tool_rejects_feedback_over_the_mcp_transport() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("Graph.auris");
        let mut session =
            auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
        let a = session.add_bus_track("A");
        let b = session.add_bus_track("B");
        session.save(&path).unwrap();
        drop(session);
        let (server_transport, client_transport) = tokio::io::duplex(32768);
        let server = tokio::spawn(async move {
            AurisMcp::default()
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let forward = serde_json::json!({"project":path,"operation":{"action":"connect","source":format!("track:{}",a.0),"target":format!("track:{}",b.0),"kind":"output"}});
        let accepted = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("mixer_graph")
                    .with_arguments(forward.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_ne!(accepted.is_error, Some(true));
        let saved = std::fs::read(&path).unwrap();
        let backward = serde_json::json!({"project":path,"operation":{"action":"connect","source":format!("track:{}",b.0),"target":format!("track:{}",a.0),"kind":"output"}});
        let rejected = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("mixer_graph")
                    .with_arguments(backward.as_object().unwrap().clone()),
            )
            .await
            .unwrap();
        assert_eq!(rejected.is_error, Some(true));
        assert!(
            rejected.content[0]
                .as_text()
                .unwrap()
                .text
                .contains("cycle")
        );
        assert_eq!(std::fs::read(&path).unwrap(), saved);
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn routed_parameter_failures_include_field_and_recovery_guidance() {
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp::default()
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let result = client
            .call_tool(
                rmcp::model::CallToolRequestParams::new("add_track").with_arguments(
                    serde_json::json!({
                        "project":"unused.auris", "name":"Lead", "kind":["instrument"]
                    })
                    .as_object()
                    .unwrap()
                    .clone(),
                ),
            )
            .await
            .unwrap();
        assert_eq!(result.is_error, Some(true));
        let text = result
            .content
            .iter()
            .filter_map(|content| content.as_text())
            .map(|content| content.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(text.contains("arguments.kind"), "{text}");
        assert!(text.contains("string"), "{text}");
        assert!(text.contains("tool_help"), "{text}");
        client.cancel().await.unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn request_cancellation_reaches_the_blocking_worker() {
        let cancellation = std::sync::Arc::new(toolbox::Cancellation::new());
        let observed = std::sync::Arc::clone(&cancellation);
        let worker_observed = std::sync::Arc::clone(&cancellation);
        let (started, running) = std::sync::mpsc::sync_channel(0);

        let canceller = std::thread::spawn(move || {
            running.recv().expect("worker start");
            assert!(observed.cancel());
        });

        let result = REQUEST_CANCELLATION
            .scope(
                cancellation,
                run_blocking(move || {
                    started.send(()).expect("test receiver");
                    while !worker_observed.is_cancelled() {
                        std::thread::yield_now();
                    }
                    Ok::<_, String>("work completed after cancellation")
                }),
            )
            .await
            .expect("worker task");
        canceller.join().expect("canceller thread");

        assert!(result.unwrap_err().contains("cancelled"));
    }

    #[test]
    fn bad_arguments_are_tool_feedback_but_protocol_errors_keep_their_codes() {
        let result = recover_argument_error(
            Err(ErrorData::invalid_params("missing end_bar", None)),
            "edit_clip",
            true,
            None,
        )
        .unwrap();
        let rmcp::model::CallToolResponse::Complete(result) = result else {
            panic!("expected tool result")
        };
        assert_eq!(result.is_error, Some(true));
        assert!(format!("{:?}", result.content).contains("tool_help"));
        let unknown = recover_argument_error(
            Err(ErrorData::invalid_params("unknown tool", None)),
            "invented",
            false,
            None,
        )
        .unwrap_err();
        assert_eq!(unknown.code, rmcp::model::ErrorCode::INVALID_PARAMS);
        let internal = recover_argument_error(
            Err(ErrorData::internal_error("worker failed", None)),
            "edit_clip",
            true,
            None,
        )
        .unwrap_err();
        assert_eq!(internal.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
    }

    #[test]
    fn the_server_introduces_itself_and_carries_the_shared_instructions() {
        let info = AurisMcp::default().get_info();
        // The name is this crate's, not the SDK's — the `from_build_env` trap in `get_info`.
        assert_eq!(info.server_info.name, "auris-mcp");
        assert_eq!(
            info.instructions.as_deref(),
            Some(toolbox::ToolGroups::default().instructions().as_str()),
            "both doors hand a model the same standing instructions"
        );
    }

    #[tokio::test]
    async fn wire_catalog_comes_from_the_execution_registry() {
        let (server_transport, client_transport) = tokio::io::duplex(8192);
        let server = tokio::spawn(async move {
            AurisMcp::default()
                .serve(server_transport)
                .await
                .unwrap()
                .waiting()
                .await
                .unwrap();
        });
        let client = ().serve(client_transport).await.unwrap();
        let served = client.list_all_tools().await.unwrap();
        let expected = toolbox::tool_catalog();
        assert_eq!(served.len(), expected.len());
        for tool in served {
            let shared = expected
                .iter()
                .find(|entry| entry.name == tool.name.as_ref())
                .unwrap();
            let mut schema = shared.parameters.clone();
            toolbox::compact_parameters(&mut schema);
            assert_eq!(tool.input_schema.as_ref(), schema.as_object().unwrap());
        }
        client.cancel().await.unwrap();
        server.await.unwrap();
    }
}
