//! Opt-in real CLI inference for native permissions, text references, memory and Stop.

use super::*;
use crate::Worker;
use std::{fs, time::Instant};

fn preferences() -> auris_session::AgentPreferences {
    auris_session::AgentPreferences {
        backend: "opencode".into(),
        model: std::env::var("AURIS_OPENCODE_EVAL_MODEL").expect("explicit evaluation model"),
        ..Default::default()
    }
}

fn next(worker: &Worker, deadline: Instant) -> Value {
    loop {
        assert!(Instant::now() < deadline, "OpenCode evaluation timed out");
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
            // OpenCode 2 ends the turn as cancelled after an explicit permission refusal.
            "error"
                if !allow_native
                    && event["message"]
                        .as_str()
                        .is_some_and(|text| text.contains("turn did not complete: cancelled")) =>
            {
                return String::new();
            }
            "error" | "ended" => panic!("OpenCode stopped before answering: {event}"),
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
#[ignore = "real OpenCode inference; requires AURIS_OPENCODE_EVAL_MODEL and a fresh AURIS_OPENCODE_EVAL_OUTPUT"]
fn installed_opencode_checks_native_permissions_references_memory_and_stop() {
    let output = PathBuf::from(
        std::env::var_os("AURIS_OPENCODE_EVAL_OUTPUT").expect("explicit evidence directory"),
    );
    assert!(output.is_absolute());
    fs::create_dir(&output).expect("a fresh evidence directory");
    let mut events = Vec::new();
    for allowed in [false, true] {
        let folder = output.join(if allowed { "allow" } else { "deny" });
        fs::create_dir(&folder).unwrap();
        let target = folder.join("native.txt");
        let mut worker = Worker::spawn(preferences(), Some(folder), true, None).unwrap();
        worker.send(&json!({"say":format!("Native permission transport test. Use the native shell tool exactly once to run `printf auris-native-proof > '{}'`. This is a disposable test file I request you to create. Do not use any other tool or change the song. If permission is denied, stop without retry or workaround and report that denial. Otherwise report completion.", target.display())}).to_string()).unwrap();
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
    let target = folder.join("must-not-exist.txt");
    let mut worker = Worker::spawn(preferences(), Some(folder), true, None).unwrap();
    worker.send(&json!({"say":format!("Native Stop test: use the native shell tool to run `printf cancelled > '{}'`. Do not use any other tool or change the song.",target.display())}).to_string()).unwrap();
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
#[ignore = "real vision-capable OpenCode inference; requires AURIS_OPENCODE_EVAL_MODEL, AURIS_OPENCODE_EVAL_AUDIO and a fresh AURIS_OPENCODE_EVAL_OUTPUT"]
fn installed_opencode_reads_image_and_audio_references() {
    use base64::Engine;
    let output = PathBuf::from(std::env::var_os("AURIS_OPENCODE_EVAL_OUTPUT").unwrap());
    let audio = PathBuf::from(std::env::var_os("AURIS_OPENCODE_EVAL_AUDIO").unwrap());
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
