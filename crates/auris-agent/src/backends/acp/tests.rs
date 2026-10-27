//! Scripted ACP peers exercise the SDK transport and the real live document boundary.
use super::*;
use tokio::io::{AsyncBufReadExt, BufReader};

async fn reply(writer: &mut (impl AsyncWrite + Unpin), mut message: Value) {
    if message["method"] == "session/request_permission" {
        if message["params"]["toolCall"]["toolCallId"].is_null() {
            message["params"]["toolCall"]["toolCallId"] = json!("fixture");
        }
        if let Some(options) = message["params"]["options"].as_array_mut() {
            for option in options {
                option["name"] = option["optionId"].clone();
            }
        }
    }
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
    assert!(models(&json!({})).unwrap_err().contains("models"));
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

#[tokio::test]
async fn initialization_passes_only_the_authenticated_live_server() {
    for modern in [false, true] {
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
            let mut session = json!({"sessionId":"created","models":{"availableModels":[{"modelId":"provider/model","name":"Model"}]}});
            if modern {
                session["configOptions"] = json!([{"id":"agent","category":"mode","options":[{"value":"auris","name":"Auris"}]}]);
            } else {
                session["modes"] = json!({"availableModes":[{"id":"auris"}]});
            }
            reply(
                &mut writer,
                json!({"jsonrpc":"2.0","id":new["id"],"result":session}),
            )
            .await;
            let mode: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(
                mode["method"],
                if modern {
                    "session/set_config_option"
                } else {
                    "session/set_mode"
                }
            );
            assert_eq!(
                mode["params"][if modern { "value" } else { "modeId" }],
                "auris"
            );
            if modern {
                assert_eq!(mode["params"]["configId"], "agent");
            }
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
    turn.update(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"shell-id","status":"completed"}),
        Some(&bridge),
    )
    .unwrap();
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
                    reply(&mut writer, notification(json!({"sessionUpdate":"tool_call","toolCallId":"edit","title":"Update lead.txt","kind":"edit","status":"in_progress","rawInput":{"filepath":"lead.txt","diff":"+melody"}}))).await;
                    reply(&mut writer, json!({"jsonrpc":"2.0","id":0,"method":"session/request_permission","params":{
                        "sessionId":if stale { "old" } else { "session" },
                        "toolCall":{"toolCallId":"edit","title":"edit","kind":"edit","rawInput":{"filepath":"lead.txt","diff":"+melody"}},
                        "options":[{"optionId":"always","kind":"allow_always"},{"optionId":"once","kind":"allow_once"},{"optionId":"reject","kind":"reject_once"}]}})).await;
                    let response: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(response["id"], 0);
                    assert_eq!(response["result"]["outcome"]["optionId"], if allowed && !stale {"once"} else {"reject"});
                    reply(&mut writer, json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}})).await;
                };
                let owner = async {
                    if stale { return; }
                    loop {
                        match events.try_recv() {
                            Ok(event) if event["event"] == "permission" => {
                                assert_eq!(event["origin"]["call_id"], "edit");
                                assert_eq!(event["origin"]["tool"], "cli_file_change");
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

#[test]
fn native_display_titles_preserve_operation_identity_and_completed_review_evidence() {
    for (kind, operation) in [
        ("execute", "cli_command"),
        ("edit", "cli_file_change"),
        ("read", "cli_file_read"),
    ] {
        let (bridge, _commands, events) = crate::worker::test_bridge();
        let mut turn = Turn::default();
        turn.update(&json!({"sessionUpdate":"tool_call","toolCallId":"native","title":"Agent-specific presentation text","kind":kind,"status":"in_progress","rawInput":{"command":"fixture"}}), Some(&bridge)).unwrap();
        turn.update(&json!({"sessionUpdate":"tool_call_update","toolCallId":"native","status":"completed","content":[{"type":"content","content":{"type":"text","text":"Observed output"}}]}), Some(&bridge)).unwrap();
        let events: Vec<_> = events.try_iter().collect();
        let call = events
            .iter()
            .find(|event| event["event"] == "call")
            .unwrap();
        let result = events
            .iter()
            .find(|event| event["event"] == "result")
            .unwrap();
        assert_eq!(call["tool"], operation);
        assert_eq!(result["tool"], operation);
        assert_eq!(result["call_id"], call["call_id"]);
        assert_eq!(result["text"], "Observed output");
        assert_eq!(result["ok"], true);
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
            .contains("reviewer")
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
                assert!(!text.trim_start().starts_with('/'), "Auris input must not invoke ACP slash commands");
                let client = reqwest::Client::new();
                let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token).header("Accept", "application/json, text/event-stream").json(&value);
                let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"acp-fixture","version":"1"}}})).send().await.unwrap();
                let session = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
                request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session).send().await.unwrap().error_for_status().unwrap();
                let result = request(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add_track","arguments":{"name":"ACP Lead","kind":"instrument"}}})).header("mcp-session-id", &session).send().await.unwrap().text().await.unwrap();
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
                assert_eq!(song.project().tracks.iter().any(|track| track.name == "ACP Lead"), allowed);
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
#[ignore = "requires an installed, authenticated agent and explicitly selected backend"]
async fn installed_acp_initializes_models_and_live_catalog() {
    let backend = std::env::var("AURIS_ACP_EVAL_BACKEND").expect("explicit backend");
    let prefs = auris_session::AgentPreferences {
        backend,
        model: std::env::var("AURIS_ACP_EVAL_MODEL").unwrap_or_default(),
        ..Default::default()
    };
    let cwd = working_directory().unwrap();
    let (bridge, _commands, _events) = crate::worker::test_bridge();
    let mcp = LiveMcp::start(ToolHost::new(bridge)).await.unwrap();
    let (mut cli, session) = tokio::time::timeout(
        Duration::from_secs(45),
        Cli::spawn(&prefs, cwd.path(), None, Some(&mcp), true),
    )
    .await
    .expect("startup timeout")
    .expect("ACP startup");
    let selection = cli.select_model(&prefs, &session).await;
    cli.stop().await;
    selection.expect("model selection");
    println!("{}", models(&session).unwrap());
}
#[tokio::test]
async fn acp_imports_external_notes_and_respects_denied_permissions() {
    for allowed in [true, false] {
        tokio::time::timeout(Duration::from_secs(10), async {
                let (bridge, commands, events) = crate::worker::test_bridge();
                let mcp = LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
                let (mut cli, server) = transport();
                let references = tempfile::tempdir().unwrap();
                let audio = references.path().join("reference.wav");
                crate::attachments::tests::reference_wav(&audio, false);
                commands.send(json!({"say":"Add a track","attachments":[audio]}).to_string()).unwrap();
                let drive = async { conversation(&mut cli, &bridge, &mcp, None, false, None, "fixture").await.unwrap() };
                let peer = async {
                    let (reader, mut writer) = tokio::io::split(server);
                    let mut lines = BufReader::new(reader).lines();
                    let prompt: Value = serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(prompt["method"], "session/prompt");
                    assert!(prompt["params"]["prompt"][1]["text"].as_str().unwrap().contains("local CPU audio measurements"));
                    let client = reqwest::Client::new();
                    let request = |value: Value| client.post(&mcp.url).bearer_auth(&mcp.token).header("Accept", "application/json, text/event-stream").json(&value);
                    let init = request(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"claude-fixture","version":"1"}}})).send().await.unwrap();
                    let session = init.headers().get("mcp-session-id").unwrap().to_str().unwrap().to_owned();
                    request(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).header("mcp-session-id", &session).send().await.unwrap();
                    let result=crate::live_mcp::transcription_test_call(&client,&mcp.url,&mcp.token,&session,"ACP lead").await;
                    assert_eq!(result["result"]["isError"] == true, !allowed, "{result}");
                    reply(&mut writer, json!({"jsonrpc":"2.0","id":prompt["id"],"result":{"stopReason":"end_turn"}})).await;
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
async fn config_models_keep_base_ids_and_apply_the_selected_models_effort() {
    let session = json!({"models":{"availableModels":[{"modelId":"model[high]"}]},
        "configOptions":[{"id":"model","category":"model","options":[{"value":"model","name":"Model"}]},
            {"id":"effort","category":"thought_level","options":[{"value":"low"},{"value":"high"}]}]});
    let catalog: Value = serde_json::from_str(&models(&session).unwrap()).unwrap();
    assert_eq!(catalog["models"][0]["name"], "model");
    assert_eq!(catalog["models"][0]["efforts"], json!(["low", "high"]));
    let (mut cli, server) = transport();
    cli.backend = "codex".into();
    let prefs = auris_session::AgentPreferences {
        model: "model".into(),
        effort: auris_session::ReasoningEffort::Low,
        ..Default::default()
    };
    let peer = async {
        let (reader, mut writer) = tokio::io::split(server);
        let mut lines = BufReader::new(reader).lines();
        for (id, value) in [("model", "model"), ("effort", "low")] {
            let request: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert_eq!(request["method"], "session/set_config_option");
            assert_eq!(request["params"]["configId"], id);
            assert_eq!(request["params"]["value"], value);
            reply(&mut writer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"configOptions":[{"id":"effort","category":"thought_level","options":[{"value":"low"}]}]}})).await;
        }
    };
    let (result, ()) = tokio::join!(cli.select_model(&prefs, &session), peer);
    result.unwrap();
}

#[tokio::test]
async fn claude_recovery_uses_the_same_session_and_stops_after_denial_or_native_failure() {
    for (name, denied, recover) in [
        ("mcp__auris_live__add_track", false, true),
        ("mcp__auris_live__add_track", true, false),
        ("Bash", false, false),
    ] {
        tokio::time::timeout(Duration::from_secs(5),async {
            let (mut cli,server)=transport();
            cli.backend="claude".into();
            let (bridge,_commands,events)=crate::worker::test_bridge();
            if denied {bridge.block_tool_recovery();}
            let peer=async {
                let (reader,mut writer)=tokio::io::split(server);
                let mut lines=BufReader::new(reader).lines();
                let request:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                reply(&mut writer,notification(json!({"sessionUpdate":"tool_call","toolCallId":"failed","title":name,"rawInput":{"kind":"invalid"},"status":"failed"}))).await;
                reply(&mut writer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}})).await;
                if recover {
                    let request:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
                    assert_eq!(request["params"]["sessionId"],"session");
                    assert!(request["params"]["prompt"][0]["text"].as_str().unwrap().contains("unfinished original user request"));
                    reply(&mut writer,notification(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Recovered"}}))).await;
                    reply(&mut writer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}})).await;
                }
            };
            let (result,())=tokio::join!(cli.prompt(json!([]),Some(&bridge)),peer);
            assert_eq!(result.unwrap(),if recover {"Recovered"} else {""});
            assert_eq!(events.try_iter().filter(|event| event["event"]=="notice").count(),usize::from(recover));
        }).await.unwrap();
    }
}

#[tokio::test]
async fn terminal_turn_error_preserves_the_next_user_request() {
    tokio::time::timeout(Duration::from_secs(5),async {
        let (mut cli,server)=transport();
        let (bridge,commands,events)=crate::worker::test_bridge();
        let mcp=LiveMcp::start(ToolHost::new(bridge.clone())).await.unwrap();
        commands.send(json!({"say":"First"}).to_string()).unwrap();
        commands.send(json!({"say":"Second"}).to_string()).unwrap();
        let peer=async {
            let (reader,mut writer)=tokio::io::split(server);
            let mut lines=BufReader::new(reader).lines();
            let request:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            reply(&mut writer,json!({"jsonrpc":"2.0","id":request["id"],"error":{"code":-32603,"message":"Fixture turn error"}})).await;
            let request:Value=serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            assert!(request["params"]["prompt"][0]["text"].as_str().unwrap().contains("Second"));
            reply(&mut writer,notification(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Still connected"}}))).await;
            reply(&mut writer,json!({"jsonrpc":"2.0","id":request["id"],"result":{"stopReason":"end_turn"}})).await;
            drop(commands);
        };
        let (result,())=tokio::join!(conversation(&mut cli,&bridge,&mcp,None,false,None,"fixture"),peer);
        result.unwrap();
        let events:Vec<_>=events.try_iter().collect();
        assert_eq!(events.iter().filter(|event| event["event"]=="error").count(),1);
        assert!(events.iter().any(|event| event["event"]=="answer" && event["text"]=="Still connected"));
    }).await.unwrap();
}
