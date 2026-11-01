//! The transport-independent live tool path. Only the desktop owns the session.

use crate::{Bridge, permissions, toolbox};
use serde_json::{Value, json};

/// One window's tool endpoint, shared by rig, Antigravity and the embedded MCP server.
#[derive(Clone)]
pub(crate) struct ToolHost {
    bridge: Bridge,
}

impl ToolHost {
    pub(crate) fn new(bridge: Bridge) -> Self {
        Self { bridge }
    }

    pub(crate) fn search_available() -> bool {
        std::env::var("BRAVE_SEARCH_API_KEY").is_ok_and(|key| !key.trim().is_empty())
    }

    pub(crate) fn catalog() -> Vec<toolbox::live_agent::Definition> {
        let mut catalog = toolbox::live_agent::definitions();
        // Candidate schemas add about 9 KiB. Keep the experimental surface explicit so
        // existing 32k local-model conversations retain their inspection headroom.
        if !std::env::var("AURIS_COMPOSITION_CANDIDATES").is_ok_and(|v| v == "1") {
            catalog.retain(|tool| !matches!(tool.name.as_str(), "candidates" | "phrase_variation"));
        }
        if Self::search_available() {
            catalog.push(toolbox::live_agent::Definition {
                name: toolbox::internet_search::NAME.into(),
                description: toolbox::internet_search::DESCRIPTION.into(),
                parameters: toolbox::parameter_schema::<toolbox::internet_search::Args>(),
            });
        }
        catalog
    }

    pub(crate) fn definitions() -> Vec<toolbox::live_agent::Definition> {
        Self::catalog()
            .into_iter()
            .map(|mut tool| {
                toolbox::compact_parameters(&mut tool.parameters);
                // Full contracts stay available through tool_help; schema growth must not
                // crowd ordinary requests out of the default native-model context.
                let concise = toolbox::concise_description(&tool.description);
                let sentence = concise.split(". ").next().unwrap_or_default();
                tool.description = sentence.to_owned();
                tool
            })
            .collect()
    }

    pub(crate) fn contains(name: &str) -> bool {
        Self::catalog().iter().any(|tool| tool.name == name)
    }

    pub(crate) async fn call_with_origin(
        &self,
        name: &str,
        args: Value,
        origin: permissions::PermissionOrigin,
    ) -> Result<Value, String> {
        permissions::with_origin(origin, self.call(name, args)).await
    }

    pub(crate) async fn traced_call(&self, name: &str, args: Value) -> Result<Value, String> {
        // MCP request identities are distinct from a CLI's model-side tool IDs.
        let id = format!("live-mcp-{}", uuid::Uuid::new_v4());
        self.bridge
            .emit(json!({"event":"trace_call","call_id":id,"tool":name}));
        let result = self
            .call_with_origin(
                name,
                args,
                permissions::call_origin(Some(&id), name, "live_mcp"),
            )
            .await;
        let ok = result.as_ref().is_ok_and(|reply| reply["ok"] == true);
        self.bridge
            .emit(json!({"event":"trace_result","call_id":id,"tool":name,"ok":ok}));
        result
    }

    pub(crate) async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        // Keep approval and execution together even when an MCP client calls in parallel.
        let _guard = self.bridge.tool_lock().await;
        self.bridge.reserve_tool_call()?;
        let result = self.call_reserved(name, args).await;
        // A model that scripts its calls cannot see the allowance being spent; near the end,
        // every answer says what is left.
        let Some(left) = self.bridge.tool_calls_left_if_low() else {
            return result;
        };
        match result {
            Ok(mut reply) => {
                if let Some(text) = reply["text"].as_str() {
                    reply["text"] = crate::tool_calls::with_remaining(text, left).into();
                }
                Ok(reply)
            }
            Err(error) => Err(crate::tool_calls::with_remaining(&error, left)),
        }
    }

    async fn call_reserved(&self, name: &str, args: Value) -> Result<Value, String> {
        if !Self::contains(name) {
            return Err(format!("Unknown live tool: {name}"));
        }
        if name == toolbox::live_agent::production_record::NAME {
            let request: toolbox::live_agent::production_record::Args =
                serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            permissions::authorize(Some(&self.bridge), name, &args.to_string()).await?;
            permissions::authorize(Some(&self.bridge), "inspect_project", "{}").await?;
            let inspected = self
                .bridge
                .exchange(json!({"event":"edit","command":{"action":"inspect"}}))
                .await?;
            if inspected["ok"] != true {
                return Err(
                    "Could not reconcile production record with the current document".into(),
                );
            }
            let mut state: Value =
                serde_json::from_str(inspected["text"].as_str().ok_or("Missing inspection")?)
                    .map_err(|e| e.to_string())?;
            self.bridge.inspected_production_document(&mut state)?;
            let view = self.bridge.production_record(request.operation)?;
            return Ok(json!({"event":"edit_result","ok":true,"text":view.to_string()}));
        }
        if toolbox::runtime::live_references()
            .iter()
            .any(|tool| tool.name == name)
        {
            permissions::authorize(Some(&self.bridge), name, &args.to_string()).await?;
            let result = toolbox::runtime::call_live_reference_in(name, args, &Self::catalog())
                .expect("registered reference tool");
            return result.map(|text| json!({"event":"edit_result","ok":true,"text":text}));
        }
        if name == toolbox::internet_search::NAME {
            let request: toolbox::internet_search::Args =
                serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            permissions::authorize(Some(&self.bridge), name, &args.to_string()).await?;
            let text = crate::search_internet(&request).await?;
            return Ok(json!({"event":"edit_result","ok":true,"text":text}));
        }
        if name == "edit_notes" && args["input"]["kind"] == "file" {
            let request: toolbox::score_input::Args =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            permissions::authorize(
                Some(&self.bridge),
                "cli_file_read",
                &json!({"path":request.source_path(),"purpose":"Read exact note edits"})
                    .to_string(),
            )
            .await?;
            let command = tokio::task::spawn_blocking(move || request.command())
                .await
                .map_err(|e| e.to_string())??;
            permissions::authorize(
                Some(&self.bridge),
                "edit_project",
                &json!({"command":command}).to_string(),
            )
            .await?;
            return self
                .bridge
                .exchange(json!({"event":"edit","command":command}))
                .await;
        }
        if name == toolbox::live_agent::audio_attachment::NAME {
            let args: toolbox::live_agent::audio_attachment::Args =
                serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            let source = self.bridge.audio_reference(&args.attachment)?;
            permissions::authorize(
                Some(&self.bridge),
                name,
                &json!({"attachment":args.attachment}).to_string(),
            )
            .await?;
            let control = self.bridge.analysis_control();
            let copy = tokio::task::spawn_blocking(move || {
                auris_session::copy_audio_attachment(source.path(), &control)
                    .map_err(|e| e.to_string())
            })
            .await
            .map_err(|e| e.to_string())??;
            if self.bridge.analysis_control().is_cancelled() {
                return Err("Audio reference export cancelled".into());
            }
            let copy = self.bridge.retain_audio_export(&args.attachment, copy)?;
            let metadata = json!({"attachment_id":args.attachment,"audio_path":copy.path(),
                "sha256":copy.sha256,"bytes":copy.bytes,"representation":"exact original audio bytes; no resampling or channel conversion",
                "lifetime":"Until the next export of this attachment, its expiry, or conversation shutdown. Copy it to your analysis workspace if longer retention is needed.",
                "status":"Local file access only; not proof of listening or transcription."});
            return Ok(json!({"event":"edit_result","ok":true,"text":metadata.to_string()}));
        }
        if name == toolbox::live_agent::transcription_import::NAME {
            let request: toolbox::live_agent::transcription_import::Args =
                serde_json::from_value(args).map_err(|e| e.to_string())?;
            request.validate()?;
            if let Some(path) = &request.path {
                permissions::authorize(
                    Some(&self.bridge),
                    "cli_file_read",
                    &json!({"path":path,"purpose":"Read external transcription JSON"}).to_string(),
                )
                .await?;
            }
            let control = self.bridge.analysis_control();
            let command = tokio::task::spawn_blocking(move || request.command(&control))
                .await
                .map_err(|e| e.to_string())??;
            permissions::authorize(
                Some(&self.bridge),
                "edit_project",
                &json!({"command":command}).to_string(),
            )
            .await?;
            return self
                .bridge
                .exchange(json!({"event":"edit","command":command}))
                .await;
        }
        let command = toolbox::live_agent::command(name, &args)?
            .ok_or_else(|| format!("Unknown live tool: {name}"))?;
        permissions::authorize(Some(&self.bridge), name, &args.to_string()).await?;
        if let auris_session::live_agent::Command::RepairLyrics { request } = &command {
            self.bridge.reserve_lyric_repair(request)?;
        }
        let mut reply = self
            .bridge
            .exchange(json!({"event":"edit", "command":command}))
            .await?;
        if reply["event"] != "edit_result" {
            return Err("The edit response did not match the pending request".into());
        }
        if reply["ok"] == true && matches!(name, "inspect_project" | "listen") {
            let mut value: Value =
                serde_json::from_str(reply["text"].as_str().ok_or("Missing tool response")?)
                    .map_err(|e| e.to_string())?;
            if name == "inspect_project" {
                self.bridge.inspected_production_document(&mut value)?;
                reply["text"] = value.to_string().into();
            } else if let Some(revision) = value["revision"].as_u64() {
                self.bridge.observe_production(
                    name,
                    revision,
                    args["start_bar"].as_u64().zip(args["bars"].as_u64()),
                    None,
                    value,
                )?;
            }
        }
        if matches!(
            name,
            "analyze_lyrics"
                | "diagnose_song"
                | "repair_lyrics"
                | "arrangement"
                | "candidates"
                | "phrase_variation"
        ) && reply["ok"] == true
            && let Some(text) = reply["text"].as_str()
        {
            reply["text"] = toolbox::present_domain_report(text)?.into();
        }
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn production_reads_reinspect_the_document_and_share_permissions() {
        let (bridge, commands, events) = crate::worker::test_bridge();
        let host = ToolHost::new(bridge);
        let call = host.call("production_record", json!({"operation":{"kind":"read"}}));
        let owner = async {
            for expected in ["production_record", "edit_project"] {
                let permission = next_event(&events).await;
                assert_eq!(permission["tool"], expected);
                commands
                    .send(
                        json!({"event":"permission_result","id":permission["id"],"ok":true})
                            .to_string(),
                    )
                    .unwrap();
            }
            assert_eq!(next_event(&events).await["command"]["action"], "inspect");
            commands.send(json!({"event":"edit_result","ok":true,"text":json!({"revision":"today","document_instance":"instance"}).to_string()}).to_string()).unwrap();
        };
        let (result, ()) = tokio::join!(call, owner);
        let view: Value = serde_json::from_str(result.unwrap()["text"].as_str().unwrap()).unwrap();
        assert_eq!(view["persistence"], "worker_lifetime");
        for kind in ["read", "remove"] {
            let operation = auris_session::agent_policy::Operation::parse(
                "production_record",
                &json!({"operation":{"kind":kind,"key":"motif"}}),
            )
            .unwrap();
            assert_eq!(operation.mutating, kind != "read");
        }
    }

    #[tokio::test]
    async fn tool_call_limit_counts_reference_calls_and_refuses_edits_before_permission() {
        let (bridge, commands, events) = crate::worker::test_bridge();
        bridge.set_test_tool_call_limit(1);
        let host = ToolHost::new(bridge.clone());
        let owner = async {
            let request = next_event(&events).await;
            assert_eq!(request["event"], "permission");
            commands
                .send(json!({"event":"permission_result","id":request["id"],"ok":true}).to_string())
                .unwrap();
        };
        let (reply, ()) = tokio::join!(host.call("capabilities", json!({})), owner);
        assert!(reply.unwrap()["ok"] == true);
        let error = host
            .call(
                "add_track",
                json!({"name":"Must not run","kind":"instrument"}),
            )
            .await
            .unwrap_err();
        assert_eq!(crate::tool_calls::limit_from_error(&error), Some(1));
        assert!(events.try_recv().is_err());

        commands
            .send(json!({"say":"Continue"}).to_string())
            .unwrap();
        bridge.receive().await.unwrap();
        bridge.reserve_tool_call().unwrap();
    }

    #[tokio::test]
    async fn performance_tools_use_the_shared_live_permission_and_undo_path() {
        for (name, args, mutating) in [
            ("inspect_performance", json!({"clip":2}), false),
            (
                "set_performance",
                json!({"clip":2,"settings":{"gate":0.5,"swing_percent":62}}),
                true,
            ),
        ] {
            for allowed in [false, true] {
                let (bridge, commands, events) = crate::worker::test_bridge();
                let host = ToolHost::new(bridge);
                let mut song = auris_session::Session::new(
                    auris_session::SessionOptions::headless().with_balance(false),
                )
                .unwrap();
                let track = song.add_default_instrument_track("Lead").unwrap();
                let clip = song
                    .add_midi_clip(
                        track,
                        "Phrase",
                        auris_session::prelude::Ticks::ZERO,
                        auris_session::prelude::Ticks::QUARTER * 4,
                    )
                    .unwrap();
                let mut args = args.clone();
                args["clip"] = json!(clip.0);
                let before = song.project().clone();
                let call = host.call(name, args);
                let owner = async {
                    let request = next_event(&events).await;
                    assert_eq!(request["event"], "permission");
                    let operation = auris_session::agent_policy::Operation::parse(
                        request["tool"].as_str().unwrap(),
                        &request["args"],
                    )
                    .unwrap();
                    assert_eq!(operation.mutating, mutating);
                    commands
                        .send(
                            json!({"event":"permission_result","id":request["id"],"ok":allowed})
                                .to_string(),
                        )
                        .unwrap();
                    if allowed {
                        let request = next_event(&events).await;
                        assert_eq!(request["event"], "edit");
                        let text = song
                            .agent_command(
                                serde_json::from_value(request["command"].clone()).unwrap(),
                            )
                            .unwrap();
                        let state: Value = serde_json::from_str(&text).unwrap();
                        assert_eq!(
                            state["settings"]["gate"],
                            if mutating { json!(0.5) } else { json!(1.0) }
                        );
                        commands
                            .send(json!({"event":"edit_result","ok":true,"text":text}).to_string())
                            .unwrap();
                        if mutating {
                            song.undo();
                        }
                    }
                    assert_eq!(song.project(), &before);
                };
                let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    tokio::join!(call, owner)
                })
                .await
                .unwrap();
                assert_eq!(result.is_ok(), allowed);
                assert!(events.try_recv().is_err());
            }
        }
    }

    async fn next_event(events: &std::sync::mpsc::Receiver<Value>) -> Value {
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                match events.try_recv() {
                    Ok(event) => return event,
                    Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                    Err(error) => panic!("{error}"),
                }
            }
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn listening_authorization_precedes_live_snapshot_exchange() {
        for allowed in [false, true] {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let host = ToolHost::new(bridge);
            let args = json!({"start_bar":1,"bars":2,"focus":"balance","compare_to":null});
            let call = host.call("listen", args);
            let owner = async {
                let request = next_event(&events).await;
                assert_eq!(request["event"], "permission");
                let operation = auris_session::agent_policy::Operation::parse(
                    request["tool"].as_str().unwrap(),
                    &request["args"],
                )
                .unwrap();
                assert_eq!(operation.name, "edit_project.listen");
                assert!(!operation.mutating && operation.confirm);
                commands
                    .send(
                        json!({"event":"permission_result","id":request["id"],"ok":allowed})
                            .to_string(),
                    )
                    .unwrap();
                if allowed {
                    let request = next_event(&events).await;
                    assert_eq!(request["event"], "edit");
                    assert_eq!(request["command"]["action"], "listen");
                    assert!(request["command"].get("project").is_none());
                    commands.send(json!({"event":"edit_result","ok":true,"text":"{\"revision\":1,\"assessment\":null}"}).to_string()).unwrap();
                }
            };
            let (result, ()) = tokio::join!(call, owner);
            assert_eq!(result.is_ok(), allowed);
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn audio_export_is_authorized_isolated_and_replaced_on_reexport() {
        let (bridge, commands, events) = crate::worker::test_bridge();
        let source = tempfile::Builder::new().suffix(".wav").tempfile().unwrap();
        std::fs::write(source.path(), b"original audio bytes").unwrap();
        let source = std::sync::Arc::new(source);
        let id = bridge.retain_audio(source.clone(), 20).unwrap();
        let host = ToolHost::new(bridge.clone());
        let mut previous: Option<std::path::PathBuf> = None;
        for allowed in [false, true, true] {
            let call = host.call("export_audio_attachment", json!({"attachment":id}));
            let owner = async {
                let event = next_event(&events).await;
                assert_eq!(event["event"], "permission");
                let operation = auris_session::agent_policy::Operation::parse(
                    event["tool"].as_str().unwrap(),
                    &event["args"],
                )
                .unwrap();
                assert_eq!(operation.name, "export_audio_attachment");
                assert!(!operation.mutating);
                commands
                    .send(
                        json!({"event":"permission_result","id":event["id"],"ok":allowed})
                            .to_string(),
                    )
                    .unwrap();
            };
            let (reply, ()) = tokio::join!(call, owner);
            assert_eq!(reply.is_ok(), allowed);
            if allowed {
                let metadata: Value =
                    serde_json::from_str(reply.unwrap()["text"].as_str().unwrap()).unwrap();
                let path = std::path::PathBuf::from(metadata["audio_path"].as_str().unwrap());
                assert_ne!(path, source.path());
                assert_eq!(std::fs::read(&path).unwrap(), b"original audio bytes");
                assert_eq!(metadata["bytes"], 20);
                assert_eq!(
                    metadata["sha256"],
                    "65ef9399e69bb351938b941f212a02f6f3e428b5483190abfe26aee57c0af072"
                );
                if let Some(old) = previous.take() {
                    assert!(!old.exists());
                }
                std::fs::write(&path, b"mutated analysis copy").unwrap();
                assert_eq!(
                    std::fs::read(source.path()).unwrap(),
                    b"original audio bytes"
                );
                previous = Some(path);
            }
            assert!(events.try_recv().is_err());
        }
        drop(host);
        drop(bridge);
        assert!(!previous.unwrap().exists());
    }

    #[tokio::test]
    async fn external_draft_file_read_and_exact_edit_have_separate_permission_gates() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("notes.json");
        let draft = json!({"source":{"method":"codex.harmonic-fit"},"seconds":2,
            "notes":[{"pitch":60,"start":0.25,"end":1.5,"strength":0.65}],
            "limitations":"Uniform dynamics"});
        let draft = serde_json::to_value(
            serde_json::from_value::<auris_session::NoteTranscription>(draft).unwrap(),
        )
        .unwrap();
        std::fs::write(&path, draft.to_string()).unwrap();
        for (file, allow_read, allow_edit) in [
            (false, true, false),
            (false, true, true),
            (true, false, true),
            (true, true, false),
            (true, true, true),
        ] {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let host = ToolHost::new(bridge);
            let args = if file {
                json!({"path":path,"at_tick":3840})
            } else {
                json!({"transcription":draft,"at_tick":3840})
            };
            let call = host.call_with_origin(
                "import_transcription",
                args,
                permissions::call_origin(Some("source-call"), "import_transcription", "fixture"),
            );
            let owner = async {
                if file {
                    let event = next_event(&events).await;
                    assert_eq!(
                        event["origin"],
                        json!({"kind":"tool_call","call_id":"source-call","tool":"import_transcription"})
                    );
                    assert_eq!(event["tool"], "cli_file_read");
                    commands
                        .send(
                            json!({"event":"permission_result","id":event["id"],"ok":allow_read})
                                .to_string(),
                        )
                        .unwrap();
                    if !allow_read {
                        return;
                    }
                }
                let event = next_event(&events).await;
                assert_eq!(event["event"], "permission");
                assert_eq!(
                    event["origin"],
                    json!({"kind":"tool_call","call_id":"source-call","tool":"import_transcription"})
                );
                assert_eq!(event["args"]["command"]["draft"]["transcription"], draft);
                let operation = auris_session::agent_policy::Operation::parse(
                    event["tool"].as_str().unwrap(),
                    &event["args"],
                )
                .unwrap();
                assert_eq!(operation.name, "edit_project.import_transcription");
                assert!(operation.mutating);
                let policy = auris_session::agent_policy::Policy {
                    mode: auris_session::agent_policy::Mode::ReadOnly,
                    ..Default::default()
                };
                assert!(matches!(
                    policy.decide(&operation),
                    auris_session::agent_policy::Decision::Deny(_)
                ));
                commands
                    .send(
                        json!({"event":"permission_result","id":event["id"],"ok":allow_edit})
                            .to_string(),
                    )
                    .unwrap();
                if !allow_edit {
                    return;
                }
                let edit = next_event(&events).await;
                assert_eq!(edit["command"], event["args"]["command"]);
                let mut session =
                    auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
                let before = session.project().clone();
                let text = session
                    .agent_command(serde_json::from_value(edit["command"].clone()).unwrap())
                    .unwrap();
                assert_eq!(session.project().tracks.len(), 1);
                session.undo();
                assert_eq!(session.project(), &before);
                commands
                    .send(json!({"event":"edit_result","ok":true,"text":text}).to_string())
                    .unwrap();
            };
            let (result, ()) = tokio::join!(call, owner);
            assert_eq!(result.is_ok(), allow_read && allow_edit);
            assert!(events.try_recv().is_err());
        }
        for invalid in [
            json!({}),
            json!({"transcription":draft,"path":path}),
            json!({"transcription":draft,"at_tick":-1}),
            json!({"transcription":draft,"name":" "}),
        ] {
            let (bridge, _, events) = crate::worker::test_bridge();
            assert!(
                ToolHost::new(bridge)
                    .call("import_transcription", invalid)
                    .await
                    .is_err()
            );
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn live_vocal_and_harmony_calls_reach_the_canonical_permission_gate_before_edits() {
        for (name, args) in [
            (
                "edit_harmony",
                json!({"start_bar":1,"bars":4,"chords":"| I | IV | V | I |"}),
            ),
            (
                "set_singer_voice",
                json!({"track":1,"voice":"/voice/test.voicevox.json"}),
            ),
            ("write_lyrics", json!({"clip":2,"text":"さくら"})),
            (
                "repair_lyrics",
                json!({"target":{"clip":2,"notes":[0]},"expected_revision":"r1","candidates":[{"text":"あ"}],"apply":true}),
            ),
            ("sing", json!({"track":1})),
        ] {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let host = ToolHost::new(bridge);
            let call = host.call(name, args);
            let owner = async {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                let event = loop {
                    assert!(std::time::Instant::now() < deadline);
                    match events.try_recv() {
                        Ok(event) => break event,
                        Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                        Err(e) => panic!("{e}"),
                    }
                };
                assert_eq!(event["event"], "permission");
                let operation = auris_session::agent_policy::Operation::parse(
                    event["tool"].as_str().unwrap(),
                    &event["args"],
                )
                .unwrap();
                assert_eq!(operation.name, format!("edit_project.{name}"));
                assert!(operation.mutating);
                commands.send(json!({"event":"permission_result","id":event["id"],"ok":false,"reason":"Read-only"}).to_string()).unwrap();
            };
            let (result, ()) = tokio::join!(call, owner);
            assert!(result.is_err());
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn external_note_import_respects_the_edit_gate() {
        for allowed in [false, true] {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let host = ToolHost::new(bridge.clone());
            let invoke = async {
                host.call(
                    "import_transcription",
                    json!({"transcription":crate::live_mcp::transcription_fixture(),"at_tick":3840}),
                )
                .await
            };
            let owner = async {
                let mut session =
                    auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
                session.add_default_instrument_track("Existing").unwrap();
                let before = session.project().clone();
                let acceptance = next_event(&events).await;
                let operation = auris_session::agent_policy::Operation::parse(
                    "edit_project",
                    &acceptance["args"],
                )
                .unwrap();
                assert_eq!(operation.name, "edit_project.import_transcription");
                assert!(operation.mutating);
                assert_eq!(session.project(), &before);
                commands
                    .send(
                        json!({"event":"permission_result","id":acceptance["id"],"ok":allowed})
                            .to_string(),
                    )
                    .unwrap();
                if allowed {
                    let edit = next_event(&events).await;
                    assert_eq!(edit["command"], acceptance["args"]["command"]);
                    let text = session
                        .agent_command(serde_json::from_value(edit["command"].clone()).unwrap())
                        .unwrap();
                    let added = session
                        .project()
                        .tracks
                        .last()
                        .unwrap()
                        .kind
                        .as_instrument()
                        .unwrap();
                    for pitch in [60, 64, 67] {
                        assert!(added.clips[0].notes.iter().any(|n| n.pitch == pitch));
                    }
                    commands
                        .send(json!({"event":"edit_result","ok":true,"text":text}).to_string())
                        .unwrap();
                    session.undo();
                }
                assert_eq!(session.project(), &before);
            };
            let (reply, ()) = tokio::time::timeout(std::time::Duration::from_secs(15), async {
                tokio::join!(invoke, owner)
            })
            .await
            .unwrap();
            assert_eq!(reply.is_ok(), allowed);
            assert!(events.try_recv().is_err());
        }
    }

    #[tokio::test]
    async fn denied_and_mismatched_approvals_never_request_an_edit() {
        for response in [
            json!({"ok":false,"reason":"Denied by the current policy"}),
            json!({"ok":true,"id":0}),
        ] {
            let (bridge, commands, events) = crate::worker::test_bridge();
            let host = ToolHost::new(bridge);
            let call = host.call("add_track", json!({"name":"Denied","kind":"instrument"}));
            let owner = async {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
                let request = loop {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "No permission request arrived"
                    );
                    match events.try_recv() {
                        Ok(event) => break event,
                        Err(std::sync::mpsc::TryRecvError::Empty) => tokio::task::yield_now().await,
                        Err(error) => panic!("{error}"),
                    }
                };
                assert_eq!(request["event"], "permission");
                assert_eq!(request["tool"], "edit_project");
                let mut reply = response.clone();
                reply["event"] = json!("permission_result");
                if reply.get("id").is_none() {
                    reply["id"] = request["id"].clone();
                }
                commands.send(reply.to_string()).unwrap();
            };
            let (result, ()) = tokio::join!(call, owner);
            assert!(result.is_err());
            assert!(events.try_recv().is_err());
        }
    }
}
