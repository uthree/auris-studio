//! Opt-in real CLI inference for native permissions, text references, memory and Stop.

use crate::Worker;
use serde_json::{Value, json};
use std::{fs, time::Instant};
use std::{path::PathBuf, time::Duration};

fn preferences() -> auris_session::AgentPreferences {
    auris_session::AgentPreferences {
        backend: std::env::var("AURIS_ACP_EVAL_BACKEND").expect("explicit evaluation backend"),
        model: std::env::var("AURIS_ACP_EVAL_MODEL").expect("explicit evaluation model"),
        effort: auris_session::ReasoningEffort::Low,
        ..Default::default()
    }
}

fn next(worker: &Worker, deadline: Instant) -> Value {
    loop {
        assert!(Instant::now() < deadline, "ACP evaluation timed out");
        match worker.try_recv() {
            Ok(event) => return event,
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("{error}"),
        }
    }
}

fn answer(worker: &Worker, allow_native: bool, events: &mut Vec<Value>) -> String {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let event = next(worker, deadline);
        events.push(event.clone());
        if let Some(root) = std::env::var_os("AURIS_ACP_EVAL_OUTPUT") {
            fs::write(
                PathBuf::from(root).join("progress.json"),
                serde_json::to_vec_pretty(events).unwrap(),
            )
            .unwrap();
        }
        match event["event"].as_str().unwrap_or_default() {
            "permission" => {
                assert!(
                    matches!(
                        event["tool"].as_str(),
                        Some("cli_command" | "cli_file_read" | "cli_file_change")
                    ),
                    "unexpected operation: {event}"
                );
                worker
                    .send(
                        &json!({"event":"permission_result","id":event["id"],"ok":allow_native})
                            .to_string(),
                    )
                    .unwrap();
            }
            "edit" => panic!("This fixture must not access the song: {event}"),
            // Some agents end the turn as cancelled after an explicit permission refusal.
            "error"
                if !allow_native
                    && event["message"]
                        .as_str()
                        .is_some_and(|text| text.contains("turn did not complete: cancelled")) =>
            {
                return String::new();
            }
            "error" | "ended" => panic!("ACP stopped before answering: {event}"),
            "answer" => return event["text"].as_str().unwrap().into(),
            _ => {}
        }
    }
}

fn stop(worker: &mut Worker, events: &mut Vec<Value>) {
    worker.cancel();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = next(worker, deadline);
        assert_ne!(event["event"], "edit", "late edit after Stop");
        let ended = event["event"] == "ended";
        events.push(event);
        if ended {
            break;
        }
    }
    assert!(worker.send(r#"{"say":"too late"}"#).is_err());
}

#[test]
#[ignore = "real ACP inference; requires AURIS_ACP_EVAL_MODEL and a fresh AURIS_ACP_EVAL_OUTPUT"]
fn installed_acp_checks_native_permissions_references_memory_and_stop() {
    let output = PathBuf::from(
        std::env::var_os("AURIS_ACP_EVAL_OUTPUT").expect("explicit evidence directory"),
    );
    assert!(output.is_absolute());
    fs::create_dir(&output).expect("a fresh evidence directory");
    let mut events = Vec::new();
    for allowed in [false, true] {
        let folder = output.join(if allowed { "allow" } else { "deny" });
        fs::create_dir(&folder).unwrap();
        let target = folder.join("native.txt");
        let mut worker = Worker::spawn(preferences(), Some(folder), true, None).unwrap();
        worker.send(&json!({"say":format!("Native permission transport test. Use the native shell tool exactly once to create the file at {} containing exactly auris-native-proof, without a trailing newline. This is a disposable test file I request you to create. Do not use any other tool or change the song. If permission is denied, stop without retry or workaround and report that denial. Otherwise report completion.", target.display())}).to_string()).unwrap();
        answer(&worker, allowed, &mut events);
        assert!(
            events
                .iter()
                .any(|event| event["event"] == "permission" && event["tool"] == "cli_command")
        );
        assert_eq!(target.exists(), allowed);
        if allowed {
            assert_eq!(fs::read_to_string(&target).unwrap(), "auris-native-proof");
        }
        stop(&mut worker, &mut events);
        fs::write(
            output.join(if allowed { "allow.json" } else { "deny.json" }),
            serde_json::to_vec_pretty(&events).unwrap(),
        )
        .unwrap();
        events.clear();
    }
    let folder = output.join("memory");
    fs::create_dir(&folder).unwrap();
    let marker = format!("auris-{}", uuid::Uuid::new_v4());
    let reference = folder.join("reference.txt");
    fs::write(
        &reference,
        format!("The test melody's private reference tag is {marker}."),
    )
    .unwrap();
    let mut worker = Worker::spawn(preferences(), Some(folder.clone()), true, None).unwrap();
    worker.send(&json!({"say":"Read the attached text, repeat its reference tag verbatim and remember it for the next request. Do not use tools or change the song.","attachments":[reference]}).to_string()).unwrap();
    assert!(answer(&worker, false, &mut events).contains(&marker));
    stop(&mut worker, &mut events);
    let mut worker = Worker::spawn(preferences(), Some(folder), false, None).unwrap();
    worker.send(&json!({"say":"What was the exact private reference tag in my earlier attachment? Answer from the conversation history only. Do not use tools."}).to_string()).unwrap();
    assert!(answer(&worker, false, &mut events).contains(&marker));
    worker.send(r#"{"compact":true}"#).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = next(&worker, deadline);
        let compacted = event["event"] == "compacted";
        if compacted {
            assert_eq!(event["ok"], false);
            assert_eq!(event["manual"], true);
        }
        events.push(event);
        if compacted {
            break;
        }
    }
    stop(&mut worker, &mut events);
    fs::write(
        output.join("memory.json"),
        serde_json::to_vec_pretty(&events).unwrap(),
    )
    .unwrap();
    events.clear();
    let folder = output.join("cancel");
    fs::create_dir(&folder).unwrap();
    let target = folder.join("pending-write.txt");
    let mut worker = Worker::spawn(preferences(), Some(folder), true, None).unwrap();
    worker.send(&json!({"say":format!("Create the disposable file at {} containing proof using the native shell tool. Do not use any other tool or change the song.",target.display())}).to_string()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let event = next(&worker, deadline);
        let pending = event["event"] == "permission";
        assert!(
            !matches!(event["event"].as_str(), Some("error" | "answer" | "ended")),
            "expected a pending permission: {event}"
        );
        events.push(event);
        if pending {
            break;
        }
    }
    stop(&mut worker, &mut events);
    assert!(
        !target.exists(),
        "a cancelled permission must never execute"
    );
    fs::write(
        output.join("cancel.json"),
        serde_json::to_vec_pretty(&events).unwrap(),
    )
    .unwrap();
}

#[test]
#[ignore = "real vision-capable ACP inference; requires AURIS_ACP_EVAL_MODEL, AURIS_ACP_EVAL_AUDIO and a fresh AURIS_ACP_EVAL_OUTPUT"]
fn installed_acp_reads_image_and_audio_references() {
    use base64::Engine;
    let output = PathBuf::from(std::env::var_os("AURIS_ACP_EVAL_OUTPUT").unwrap());
    let audio = PathBuf::from(std::env::var_os("AURIS_ACP_EVAL_AUDIO").unwrap());
    assert!(output.is_absolute() && audio.is_absolute());
    fs::create_dir(&output).expect("a fresh evidence directory");
    let image = output.join("red.png");
    // A 64x64 RGB fixture with every pixel red; no renderer or external image dependency.
    fs::write(&image, base64::engine::general_purpose::STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAEAAAABACAIAAAAlC+aJAAAAb0lEQVR4nO3PAQkAAAyEwO9feoshgnABdLep8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3I8QUNyPEFDcjxBQ3IPanc8OLDQitxAAAAAElFTkSuQmCC").unwrap()).unwrap();
    let mut worker = Worker::spawn(preferences(), None, true, None).unwrap();
    worker.send(&json!({"say":"Attachment transport test. Read the image and the audio reference metadata. Return ONLY a JSON object with colour (the dominant image colour in English), seconds (the exact numeric duration from the audio measurements) and audio_sent (whether raw audio was sent to you). These measurements are not listening. Do not call tools, read other files, or change the song.","attachments":[image,audio]}).to_string()).unwrap();
    let mut events = Vec::new();
    let text = answer(&worker, false, &mut events);
    fs::write(
        output.join("references.json"),
        serde_json::to_vec_pretty(&events).unwrap(),
    )
    .unwrap();
    let result: Value = serde_json::from_str(
        text.trim()
            .trim_start_matches("```json")
            .trim_end_matches("```")
            .trim(),
    )
    .unwrap();
    assert_eq!(
        result["colour"].as_str().unwrap().to_ascii_lowercase(),
        "red"
    );
    assert_eq!(result["seconds"].as_f64().unwrap(), 2.0);
    assert_eq!(result["audio_sent"], false);
    assert!(!events.iter().any(|event| event["event"] == "permission"));
    stop(&mut worker, &mut events);
}

#[test]
#[ignore = "real ACP inference; requires AURIS_ACP_EVAL_BACKEND, MODEL and a fresh OUTPUT directory"]
fn installed_acp_edits_unsaved_song_with_system_instructions_and_undo() {
    let output = PathBuf::from(std::env::var_os("AURIS_ACP_EVAL_OUTPUT").unwrap());
    fs::create_dir(&output).expect("a fresh evidence directory");
    let name = format!("ACP proof {}", uuid::Uuid::new_v4());
    let mut prefs = preferences();
    prefs.system_prompt = format!(
        "You edit the open Auris document. For the transport proof, call add_track exactly once with name {name} and kind instrument. Do not change anything else or use native tools. No notes or rendering are needed. Report completion."
    );
    let mut worker = Worker::spawn(prefs, None, true, None).unwrap();
    worker.send(&json!({"say":"Run the transport proof described in your system instructions.","policy":auris_session::agent_policy::Policy::default()}).to_string()).unwrap();
    let mut session =
        auris_session::Session::new(auris_session::SessionOptions::headless()).unwrap();
    let before = session.project().clone();
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut events = Vec::new();
    loop {
        let event = next(&worker, deadline);
        events.push(event.clone());
        // Save partial evidence too, so failures can be diagnosed without rerunning inference.
        fs::write(
            output.join("live-edit.json"),
            serde_json::to_vec_pretty(&events).unwrap(),
        )
        .unwrap();
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
                assert!(matches!(
                    event["command"]["action"].as_str(),
                    Some("inspect" | "add_track")
                ));
                let result = session
                    .agent_command(serde_json::from_value(event["command"].clone()).unwrap());
                worker.send(&json!({"event":"edit_result","ok":result.is_ok(),"text":result.unwrap_or_else(|error| error)}).to_string()).unwrap();
            }
            "error" | "ended" => panic!("ACP stopped before completing the edit: {event}"),
            "answer" => break,
            _ => {}
        }
    }
    assert!(
        session
            .project()
            .tracks
            .iter()
            .any(|track| track.name == name)
    );
    assert_eq!(session.project().tracks.len(), before.tracks.len() + 1);
    assert!(session.path().is_none());
    assert!(session.is_dirty());
    assert!(session.undo().is_some());
    assert_eq!(session.project(), &before);
    stop(&mut worker, &mut events);
    fs::write(
        output.join("live-edit.json"),
        serde_json::to_vec_pretty(&events).unwrap(),
    )
    .unwrap();
}
