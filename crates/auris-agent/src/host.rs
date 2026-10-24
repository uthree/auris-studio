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
                tool.description = toolbox::concise_description(&tool.description);
                tool
            })
            .collect()
    }

    pub(crate) fn contains(name: &str) -> bool {
        Self::catalog().iter().any(|tool| tool.name == name)
    }

    pub(crate) async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        // Keep approval and execution together even when an MCP client calls in parallel.
        let _guard = self.bridge.tool_lock().await;
        if !Self::contains(name) {
            return Err(format!("Unknown live tool: {name}"));
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
        if name == toolbox::transcription_tools::NAME {
            let request: toolbox::transcription_tools::Args =
                serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
            request.validate()?;
            let source = request
                .attachment
                .as_ref()
                .map(|id| self.bridge.audio_reference(id))
                .transpose()?;
            permissions::authorize(Some(&self.bridge), name, &args.to_string()).await?;
            let control = self.bridge.analysis_control();
            let text = tokio::task::spawn_blocking(move || {
                let path = source
                    .as_ref()
                    .map(|s| s.path())
                    .or_else(|| request.path.as_deref().map(std::path::Path::new))
                    .ok_or("Missing audio source")?;
                toolbox::transcription_tools::recognize(path, request.backend, &control)
            })
            .await
            .map_err(|e| e.to_string())??;
            return Ok(json!({"event":"edit_result","ok":true,"text":text}));
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
        if matches!(
            name,
            "analyze_lyrics" | "diagnose_song" | "repair_lyrics" | "arrangement"
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
                    commands.send(json!({"event":"edit_result","ok":true,"text":"scripted host response"}).to_string()).unwrap();
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
            let call = host.call("import_transcription", args);
            let owner = async {
                if file {
                    let event = next_event(&events).await;
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
    async fn recognition_uses_retained_audio_and_acceptance_has_a_separate_edit_gate() {
        for allowed in [false, true] {
            let folder = tempfile::tempdir().unwrap();
            let path = folder.path().join("reference.wav");
            crate::attachments::tests::reference_wav(&path, false);
            let (bridge, commands, events) = crate::worker::test_bridge();
            let prepared = crate::attachments::cli_input(
                "Transcribe".into(),
                vec![path.display().to_string()],
                &bridge,
                false,
            )
            .await
            .unwrap();
            let metadata: Value = serde_json::from_str(
                prepared[1]["text"]
                    .as_str()
                    .unwrap()
                    .lines()
                    .nth(1)
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&path, b"changed original").unwrap();
            let host = ToolHost::new(bridge.clone());
            let invoke = async {
                let result=host.call("transcribe_audio",json!({"attachment":metadata["attachment_id"],"backend":{"kind":"basic_pitch"}})).await.unwrap();
                let draft: Value = serde_json::from_str(result["text"].as_str().unwrap()).unwrap();
                assert!(draft["note_count"].as_u64().unwrap() >= 3);
                host.call(
                    "import_transcription",
                    json!({"draft_id":draft["draft_id"],"at_tick":3840}),
                )
                .await
            };
            let owner = async {
                let mut session =
                    auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
                session.add_default_instrument_track("Existing").unwrap();
                let before = session.project().clone();
                let recognition = next_event(&events).await;
                assert_eq!(recognition["tool"], "transcribe_audio");
                let operation = auris_session::agent_policy::Operation::parse(
                    "transcribe_audio",
                    &recognition["args"],
                )
                .unwrap();
                assert!(!operation.mutating && !operation.confirm);
                commands
                    .send(
                        json!({"event":"permission_result","id":recognition["id"],"ok":true})
                            .to_string(),
                    )
                    .unwrap();
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
            let other = ToolHost::new(crate::worker::test_bridge().0);
            assert!(other.call("transcribe_audio",json!({"attachment":metadata["attachment_id"],"backend":{"kind":"basic_pitch"}})).await.unwrap_err().contains("Unknown or expired"));
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
