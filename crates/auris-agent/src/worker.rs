//! Per-conversation channels and cancellation; no process-global session or working directory.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

static HISTORY_IO: std::sync::Mutex<()> = std::sync::Mutex::new(());
use tokio::sync::{Mutex, mpsc as async_mpsc, oneshot};

#[derive(Clone)]
pub(super) struct Bridge(Arc<Channels>);
struct Channels {
    stopped: Arc<AtomicBool>,
    events: mpsc::Sender<serde_json::Value>,
    commands: Mutex<async_mpsc::UnboundedReceiver<String>>,
    exchange: Mutex<()>,
    vision: AtomicBool,
    visual: std::sync::Mutex<Option<Message>>,
}

pub(super) struct VisualTurn(Option<Bridge>);
impl VisualTurn {
    pub(super) fn new(bridge: Option<Bridge>) -> Self {
        if let Some(bridge) = &bridge {
            bridge.clear_visual();
        }
        Self(bridge)
    }
}
impl Drop for VisualTurn {
    fn drop(&mut self) {
        if let Some(bridge) = &self.0 {
            bridge.clear_visual();
        }
    }
}

impl Bridge {
    pub(super) fn clear_visual(&self) {
        *self.0.visual.lock().unwrap() = None;
    }
    pub(super) fn visual(&self) -> Option<Message> {
        self.0.visual.lock().unwrap().clone()
    }
    pub(super) fn accept_inspection(
        &self,
        report: &auris_session::audio_inspection::Inspection,
    ) -> Result<String, String> {
        let presentation = toolbox::audio_inspection::present(report)?;
        let mut text = presentation.text;
        if self.0.vision.load(Ordering::Relaxed) {
            let image = rig::message::UserContent::image_base64(
                presentation.png,
                Some(rig::message::ImageMediaType::PNG),
                None,
            );
            let mut stored = self.0.visual.lock().unwrap();
            let mut content: Vec<_> = match stored.take() {
                Some(Message::User { content }) => content.into_iter().collect(),
                _ => Vec::new(),
            };
            if content.len() >= 4 {
                content.drain(..content.len() - 2);
            }
            content.push(rig::message::UserContent::text(format!("Historical inspection snapshot, revision {}: start_bar={}, bars={}, duration={} seconds. This may predate edits; inspect again to evaluate changed sound. Image 512x384: top 128 rows show mel power (high frequency at top, black=-90 dB, white=0 dB); bottom 256 rows show authored notes (MIDI 127 at top, 0 at bottom). Time runs left to right across the selected range. Refer to the matching inspect_audio tool result for measurements and score data.", report.revision, report.measurements["start_bar"], report.measurements["bars"], report.measurements["seconds"])));
            content.push(image);
            *stored = Some(Message::User { content });
            text.push_str("\nThe host provides the image in this request's visual context.");
        } else {
            self.clear_visual();
            text.push_str("\nImage not sent: this model has no confirmed vision capability. Use measurements and score data only.");
        }
        Ok(text)
    }
    pub(super) fn history<T>(
        &self,
        operation: impl FnOnce() -> Result<T, String>,
    ) -> Result<T, String> {
        // A replacement worker must read after any final write already in progress.
        let _guard = HISTORY_IO
            .lock()
            .map_err(|_| "Conversation storage failed")?;
        if self.0.stopped.load(Ordering::Acquire) {
            return Err("The agent worker stopped".into());
        }
        operation()
    }

    pub(super) fn emit(&self, event: serde_json::Value) {
        let _ = self.0.events.send(event);
    }
    pub(super) async fn receive(&self) -> Option<String> {
        self.0.commands.lock().await.recv().await
    }
    pub(super) async fn exchange(
        &self,
        request: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let _guard = self.0.exchange.lock().await;
        self.0
            .events
            .send(request)
            .map_err(|_| "The agent panel closed")?;
        let response = self.receive().await.ok_or("The agent panel closed")?;
        serde_json::from_str(&response).map_err(|error| error.to_string())
    }
}

/// A cancellable background conversation. Dropping it never waits on the UI thread.
pub struct Worker {
    stopped: Arc<AtomicBool>,
    commands: async_mpsc::UnboundedSender<String>,
    events: mpsc::Receiver<serde_json::Value>,
    cancel: Option<oneshot::Sender<()>>,
}

impl Worker {
    /// Start one worker using a snapshot of the host's settings and optional project folder.
    /// The host remains responsible for all permission decisions and document edits.
    pub fn spawn(
        prefs: auris_session::AgentPreferences,
        folder: Option<PathBuf>,
        fresh: bool,
    ) -> Result<Self, String> {
        let Command::Run(options) = parse_command(&[], &|name| std::env::var(name).ok(), &prefs)?
        else {
            return Err("Expected agent settings".into());
        };
        let (commands, incoming) = async_mpsc::unbounded_channel();
        let (events_out, events) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let bridge = Bridge(Arc::new(Channels {
            stopped: stopped.clone(),
            events: events_out,
            commands: Mutex::new(incoming),
            exchange: Mutex::new(()),
            vision: AtomicBool::new(false),
            visual: std::sync::Mutex::new(None),
        }));
        let (cancel, cancelled) = oneshot::channel();
        std::thread::Builder::new().name("auris-agent".into()).spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|error| error.to_string())?;
                let result = runtime.block_on(async {
                    tokio::select! {
                        biased;
                        _ = cancelled => Ok(()),
                        result = async {
                            let agent = build_for_worker(&options, Some(bridge.clone()))?;
                            let vision = runtime::preflight(&options).await?;
                            bridge.0.vision.store(vision, Ordering::Relaxed);
                            json_conversation(&agent, &options, &bridge, folder.map(|folder| folder.join(".auris-conversation.json")), fresh).await
                        } => result,
                    }
                });
                // Read-only reference work already running on blocking threads may finish,
                // but it has no session access and must not hold up cancellation.
                runtime.shutdown_timeout(std::time::Duration::from_millis(100));
                result
            })).unwrap_or_else(|_| Err("The agent worker panicked".into()));
            if let Err(message) = result { bridge.emit(serde_json::json!({"event":"error", "message":message})); }
            bridge.emit(serde_json::json!({"event":"ended"}));
        }).map_err(|error| error.to_string())?;
        Ok(Self {
            stopped,
            commands,
            events,
            cancel: Some(cancel),
        })
    }

    /// Cancel pending model and approval work without waiting for the worker thread.
    pub fn cancel(&mut self) {
        self.stopped.store(true, Ordering::Release);
        if let Some(cancel) = self.cancel.take() {
            let _ = cancel.send(());
        }
    }

    /// Send a user request or a host response without waiting for the worker.
    pub fn send(&self, message: &str) -> Result<(), String> {
        self.commands
            .send(message.into())
            .map_err(|_| "The agent worker stopped".into())
    }

    /// Poll one event; the desktop applies live edits on its own session thread.
    pub fn try_recv(&self) -> Result<serde_json::Value, mpsc::TryRecvError> {
        self.events.try_recv()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
    }
}

/// List provider models on a bounded background thread, without starting a conversation.
pub fn list_models_background(
    prefs: auris_session::AgentPreferences,
) -> mpsc::Receiver<Result<String, String>> {
    let (sender, receiver) = mpsc::channel();
    let failed = sender.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("auris-model-list".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let Command::Models(options) =
                    parse_command(&["models".into()], &|name| std::env::var(name).ok(), &prefs)?
                else {
                    return Err("Expected model listing settings".into());
                };
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|error| error.to_string())?;
                runtime.block_on(async {
                    tokio::time::timeout(MODEL_LIST_PATIENCE, list_models(&options))
                        .await
                        .map_err(|_| "The model provider did not answer in time".to_string())?
                })
            }))
            .unwrap_or_else(|_| Err("The model listing worker panicked".into()));
            let _ = sender.send(result);
        })
    {
        let _ = failed.send(Err(error.to_string()));
    }
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{completion, mock_server};
    use std::time::Duration;

    fn prefs(url: String) -> auris_session::AgentPreferences {
        auris_session::AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        }
    }

    fn until(worker: &Worker, event: &str) -> serde_json::Value {
        loop {
            let value = worker
                .events
                .recv_timeout(Duration::from_secs(3))
                .expect("worker event");
            assert_ne!(value["event"], "error", "{value}");
            if value["event"] == event {
                return value;
            }
            assert_ne!(value["event"], "ended", "worker ended before {event}");
        }
    }

    #[test]
    fn ollama_inspection_sends_images_as_user_context_only_for_vision_models() {
        for vision in [false, true] {
            let show = serde_json::json!({"capabilities":if vision {vec!["tools","vision"]} else {vec!["tools"]}}).to_string();
            let call = serde_json::json!({"model":"mock","created_at":"2026-09-12T00:00:00Z","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"inspect_audio","arguments":{"start_bar":1,"bars":1}}}]},"done":true,"done_reason":"stop"}).to_string();
            let done = serde_json::json!({"model":"mock","created_at":"2026-09-12T00:00:00Z","message":{"role":"assistant","content":"Measured the passage."},"done":true,"done_reason":"stop"}).to_string();
            let (url, requests) = mock_server(vec![show, call, done]);
            let mut preferences = prefs(url);
            preferences.provider = "ollama".into();
            let worker = Worker::spawn(preferences, None, false).unwrap();
            until(&worker, "ready");
            worker.send(r#"{"say":"Inspect one bar"}"#).unwrap();
            let permission = until(&worker, "permission");
            let operation = auris_session::agent_policy::Operation::parse(
                permission["tool"].as_str().unwrap(),
                &permission["args"],
            )
            .unwrap();
            assert!(!operation.mutating);
            worker.send(&serde_json::json!({"event":"permission_result","id":permission["id"],"ok":true}).to_string()).unwrap();
            let edit = until(&worker, "edit");
            assert_eq!(edit["command"]["action"], "inspect_audio");
            let report = auris_session::audio_inspection::Inspection {
                revision: 9,
                measurements: serde_json::json!({"seconds":2.0,"silent":true}),
                columns: 2,
                frequencies: vec![1000.0; 64],
                mel_db: vec![-90.0; 128],
                notes: vec![],
            };
            worker
                .send(
                    &serde_json::json!({"event":"edit_result","ok":true,"inspection":report})
                        .to_string(),
                )
                .unwrap();
            until(&worker, "answer");
            let requests = requests.lock().unwrap();
            assert_eq!(requests.len(), 3);
            let request: serde_json::Value = serde_json::from_str(&requests[2]).unwrap();
            let messages = request["messages"].as_array().unwrap();
            let images: Vec<_> = messages
                .iter()
                .filter(|m| m["images"].as_array().is_some_and(|v| !v.is_empty()))
                .collect();
            assert_eq!(images.len(), usize::from(vision));
            if vision {
                assert_eq!(images[0]["role"], "user");
                assert!(
                    images[0]["images"][0]
                        .as_str()
                        .unwrap()
                        .starts_with("iVBOR")
                );
                assert!(
                    images[0]["content"]
                        .as_str()
                        .unwrap()
                        .contains("revision 9")
                );
            }
            assert!(
                messages
                    .iter()
                    .filter(|m| m["role"] == "tool")
                    .all(|m| m.get("images").is_none())
            );
        }
    }

    #[test]
    fn visual_context_retains_two_snapshots_and_is_dropped_after_the_turn() {
        let (events, _) = mpsc::channel();
        let (_, incoming) = async_mpsc::unbounded_channel();
        let bridge = Bridge(Arc::new(Channels {
            stopped: Arc::new(AtomicBool::new(false)),
            events,
            commands: Mutex::new(incoming),
            exchange: Mutex::new(()),
            vision: AtomicBool::new(true),
            visual: std::sync::Mutex::new(None),
        }));
        let scope = VisualTurn::new(Some(bridge.clone()));
        for revision in 1..=3 {
            bridge
                .accept_inspection(&auris_session::audio_inspection::Inspection {
                    revision,
                    measurements: serde_json::json!({"seconds":2.0}),
                    columns: 2,
                    frequencies: vec![1000.0; 64],
                    mel_db: vec![-90.0; 128],
                    notes: vec![],
                })
                .unwrap();
        }
        let Message::User { content } = bridge.visual().unwrap() else {
            panic!()
        };
        assert_eq!(content.len(), 4);
        let text = serde_json::to_string(&content).unwrap();
        assert!(!text.contains("revision 1"));
        assert!(text.contains("revision 2"));
        assert!(text.contains("revision 3"));
        drop(scope);
        assert!(bridge.visual().is_none());
    }

    #[test]
    #[ignore = "requires AURIS_AGENT_VISION_MODEL and local Ollama"]
    fn local_vision_model_inspects_rendered_audio() {
        let model = std::env::var("AURIS_AGENT_VISION_MODEL").expect("vision model");
        let mut preferences = prefs("http://127.0.0.1:11434".into());
        preferences.provider = "ollama".into();
        preferences.model = model;
        preferences.thinking = Some(false);
        let mut session = auris_session::Session::new(
            auris_session::SessionOptions::headless().with_balance(false),
        )
        .unwrap();
        use auris_session::prelude::*;
        let track = session.add_default_instrument_track("Test tone").unwrap();
        session
            .set_track_instrument(track, "auris.synth.fm2")
            .unwrap();
        let clip = session
            .add_midi_clip(track, "Test", Ticks::ZERO, Ticks::from_beats(4.0))
            .unwrap();
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, Ticks::from_beats(2.0)))
            .unwrap();
        let before = session.project().clone();
        let worker = Worker::spawn(preferences, None, false).unwrap();
        worker.send(r#"{"say":"inspect_audioで1小節目だけを解析し、計測値と画像から分かることを短く説明してください。曲は変更しないでください。"}"#).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        let mut inspected = false;
        loop {
            let event = worker
                .events
                .recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
                .expect("model responded");
            match event["event"].as_str().unwrap_or_default() {
                "permission" => {
                    let op = auris_session::agent_policy::Operation::parse(
                        event["tool"].as_str().unwrap(),
                        &event["args"],
                    )
                    .unwrap();
                    worker.send(&serde_json::json!({"event":"permission_result","id":event["id"],"ok":!op.mutating}).to_string()).unwrap();
                }
                "edit" => {
                    let command: auris_session::live_agent::Command =
                        serde_json::from_value(event["command"].clone()).unwrap();
                    let wire = if let auris_session::live_agent::Command::InspectAudio {
                        start_bar,
                        bars,
                        track,
                    } = command
                    {
                        let report = session
                            .audio_inspection_job(start_bar, bars, track)
                            .unwrap()
                            .run(&AtomicBool::new(false))
                            .unwrap();
                        inspected = true;
                        serde_json::json!({"event":"edit_result","ok":true,"inspection":report})
                    } else {
                        serde_json::json!({"event":"edit_result","ok":true,"text":session.agent_command(command).unwrap()})
                    };
                    worker.send(&wire.to_string()).unwrap();
                }
                "error" | "ended" => panic!("{event}"),
                "answer" => {
                    println!("{event}");
                    break;
                }
                _ => {}
            }
        }
        assert!(inspected);
        assert_eq!(session.project(), &before);
        assert!(session.path().is_none());
    }

    fn edit_response() -> String {
        completion(
            r#"{"role":"assistant","tool_calls":[{"id":"edit","type":"function","function":{"name":"add_track","arguments":"{\"name\":\"Lead\",\"kind\":\"instrument\"}"}}]}"#,
            "tool_calls",
        )
    }

    /// Opt-in real-provider check: exercises the same worker, permissions and session edits
    /// as the desktop, with no display, audio device, project saving or replacement approval.
    #[test]
    #[ignore = "requires AURIS_AGENT_OLLAMA_MODEL and a running local Ollama server"]
    fn local_ollama_composes_a_song_without_tool_failures() {
        let model = std::env::var("AURIS_AGENT_OLLAMA_MODEL").expect("set the local model name");
        let mut session = auris_session::Session::new(
            auris_session::SessionOptions::headless().with_balance(false),
        )
        .unwrap();
        let worker = Worker::spawn(
            auris_session::AgentPreferences {
                provider: "ollama".into(),
                model,
                context_tokens: Some(32768),
                output_tokens: Some(4096),
                thinking: Some(false),
                ..Default::default()
            },
            None,
            false,
        )
        .unwrap();
        worker.send(&serde_json::json!({"say":"ゲームのボス戦みたいな緊張感のある重厚なオーケストラのループBGMを作ってください。"}).to_string()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(240);
        let mut failures = 0;
        let mut calls = 0;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = worker
                .events
                .recv_timeout(remaining)
                .expect("model finished within four minutes");
            match event["event"].as_str().unwrap_or_default() {
                "permission" => {
                    let operation = auris_session::agent_policy::Operation::parse(
                        event["tool"].as_str().unwrap(),
                        &event["args"],
                    )
                    .unwrap();
                    let allowed = matches!(
                        auris_session::agent_policy::Policy::default().decide(&operation),
                        auris_session::agent_policy::Decision::Allow
                    );
                    worker.send(&serde_json::json!({"event":"permission_result", "id":event["id"], "ok":allowed, "reason":"Replacement and network access are not authorized by this test"}).to_string()).unwrap();
                }
                "edit" => {
                    let command = serde_json::from_value(event["command"].clone()).unwrap();
                    let result = session.agent_command(command);
                    let ok = result.is_ok();
                    let text = result.unwrap_or_else(|e| e);
                    worker
                        .send(
                            &serde_json::json!({"event":"edit_result", "ok":ok, "text":text})
                                .to_string(),
                        )
                        .unwrap();
                }
                "call" => {
                    assert_ne!(event["tool"], "compose_song");
                    assert_ne!(event["tool"], "compose");
                    calls += 1;
                    println!("CALL {} {}", event["tool"], event["args"]);
                }
                "result" => {
                    if event["ok"] == false {
                        failures += 1;
                    }
                    println!("RESULT {} ok={}", event["tool"], event["ok"]);
                }
                "error" | "ended" => panic!("{event}"),
                "answer" => {
                    println!("ANSWER {}", event["text"]);
                    break;
                }
                _ => {}
            }
        }
        println!(
            "calls={calls}, failures={failures}, tracks={}",
            session.project().tracks.len()
        );
        assert_eq!(failures, 0);
        assert!(session.project().tracks.len() >= 2);
        assert!(session.project().tracks.iter().any(|track| {
            track
                .kind
                .note_clips()
                .is_some_and(|clips| clips.iter().any(|clip| !clip.notes.is_empty()))
        }));
        assert!(session.project().loop_enabled);
        assert!(session.path().is_none());
    }

    #[test]
    fn instrument_listing_uses_the_permission_checked_live_session() {
        let call = completion(
            r#"{"role":"assistant","tool_calls":[{"id":"sounds","type":"function","function":{"name":"list_instruments","arguments":"{\"query\":\"strings\"}"}}]}"#,
            "tool_calls",
        );
        let done = completion(
            r#"{"role":"assistant","content":"Found the live library"}"#,
            "stop",
        );
        let (url, requests) = mock_server(vec![call, done]);
        let worker = Worker::spawn(prefs(url), None, false).unwrap();
        worker.send(r#"{"say":"Find string instruments"}"#).unwrap();
        let permission = until(&worker, "permission");
        let operation = auris_session::agent_policy::Operation::parse(
            permission["tool"].as_str().unwrap(),
            &permission["args"],
        )
        .unwrap();
        assert!(!operation.mutating);
        assert_eq!(operation.name, "list_instruments");
        worker
            .send(
                &serde_json::json!({"event":"permission_result","id":permission["id"],"ok":true})
                    .to_string(),
            )
            .unwrap();
        let edit = until(&worker, "edit");
        assert_eq!(edit["command"]["action"], "list_instruments");
        assert_eq!(edit["command"]["query"], "strings");
        worker.send(&serde_json::json!({"event":"edit_result","ok":true,"text":"live-session-only-strings"}).to_string()).unwrap();
        until(&worker, "answer");
        assert!(requests.lock().unwrap()[1].contains("live-session-only-strings"));
    }

    #[test]
    fn concurrent_workers_keep_approval_and_live_edit_replies_separate() {
        let done = completion(r#"{"role":"assistant","content":"Done"}"#, "stop");
        let (first_url, first_log) = mock_server(vec![edit_response(), done.clone()]);
        let (second_url, second_log) = mock_server(vec![edit_response(), done]);
        let first = Worker::spawn(prefs(first_url), None, false).unwrap();
        let second = Worker::spawn(prefs(second_url), None, false).unwrap();
        first.send(r#"{"say":"Create the lead"}"#).unwrap();
        second.send(r#"{"say":"Create the lead"}"#).unwrap();
        let first_permission = until(&first, "permission");
        let second_permission = until(&second, "permission");
        for (worker, permission, answer) in [
            (&first, first_permission, "first document"),
            (&second, second_permission, "second document"),
        ] {
            assert_eq!(permission["tool"], "edit_project");
            worker.send(&serde_json::json!({"event":"permission_result", "id":permission["id"], "ok":true}).to_string()).unwrap();
            let edit = until(worker, "edit");
            assert_eq!(edit["command"]["name"], "Lead");
            worker
                .send(
                    &serde_json::json!({"event":"edit_result", "ok":true, "text":answer})
                        .to_string(),
                )
                .unwrap();
            assert_eq!(until(worker, "answer")["text"], "Done");
        }
        let first_log = first_log.lock().unwrap();
        let second_log = second_log.lock().unwrap();
        assert!(first_log[1].contains("first document"));
        assert!(!first_log[1].contains("second document"));
        assert!(second_log[1].contains("second document"));
        assert!(!second_log[1].contains("first document"));
    }

    #[test]
    fn cancellation_releases_an_approval_wait_without_executing_the_edit() {
        let (url, _) = mock_server(vec![edit_response()]);
        let mut worker = Worker::spawn(prefs(url), None, false).unwrap();
        worker.send(r#"{"say":"Create the lead"}"#).unwrap();
        until(&worker, "permission");
        worker.cancel();
        assert_eq!(
            worker.events.recv_timeout(Duration::from_secs(2)).unwrap()["event"],
            "ended"
        );
        assert!(worker.send(r#"{"say":"too late"}"#).is_err());
    }

    #[test]
    fn cancellation_releases_a_stalled_http_request() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (accepted, connected) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        let server = std::thread::spawn(move || {
            let (_connection, _) = listener.accept().unwrap();
            accepted.send(()).unwrap();
            let _ = wait.recv_timeout(Duration::from_secs(5));
        });
        let mut worker = Worker::spawn(prefs(url), None, false).unwrap();
        worker.send(r#"{"say":"hello"}"#).unwrap();
        connected.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.cancel();
        until(&worker, "ended");
        let _ = release.send(());
        server.join().unwrap();
    }
}
