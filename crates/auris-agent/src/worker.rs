//! Per-conversation channels and cancellation; no process-global session or working directory.
use super::*;
use std::io::Read;
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

static HISTORY_IO: std::sync::Mutex<()> = std::sync::Mutex::new(());
use tokio::sync::{Mutex, mpsc as async_mpsc, oneshot};

const HISTORY_GENERATION_BYTES: u64 = 32;

/// The reset generation one worker observed while loading its persisted conversation.
///
/// The stable lock file serialises cooperating processes. The separately replaced generation
/// file is the tombstone: clearing history advances it before removing the conversation, so a
/// worker that loaded older text can no longer publish that text after the clear.
#[derive(Clone, Debug)]
pub(super) struct HistoryGeneration {
    path: PathBuf,
    generation: u64,
    expected: memory::Memory,
}

/// One validated conversation snapshot shared by its preview and the worker that resumes it.
///
/// The memory and generation token remain opaque so a frontend cannot alter model history after
/// showing it. Passing this value to [`Worker::spawn`] guarantees that the worker uses precisely
/// the context returned by [`load_history_background`], rather than reading the file a second
/// time after provider startup.
#[derive(Clone, Debug)]
pub struct HistorySnapshot {
    memory: memory::Memory,
    generation: HistoryGeneration,
}

impl HistorySnapshot {
    /// Completed user and assistant turns, oldest first, for a transcript preview.
    pub fn turns(&self) -> Vec<(String, String)> {
        self.memory
            .turns
            .iter()
            .map(|turn| (turn.user.clone(), turn.answer.clone()))
            .collect()
    }

    /// Restored compacted context, when older turns were summarized.
    ///
    /// Frontends should show this text as context rather than as a new user instruction.
    pub fn summary(&self) -> Option<&str> {
        (!self.memory.summary.is_empty()).then_some(self.memory.summary.as_str())
    }

    fn path(&self) -> &Path {
        &self.generation.path
    }

    pub(super) fn into_parts(self) -> (memory::Memory, HistoryGeneration) {
        (self.memory, self.generation)
    }
}

impl HistoryGeneration {
    fn load(path: &Path, fresh: bool) -> Result<(memory::Memory, Self), String> {
        let _lock = history_file_lock(path)?;
        let generation = if fresh {
            advance_history_generation(path)?
        } else {
            read_history_generation(path)?
        };
        let memory = if fresh {
            memory::Memory::default()
        } else {
            memory::Memory::load(path)?
        };
        Ok((
            memory.clone(),
            Self {
                path: path.to_path_buf(),
                generation,
                expected: memory,
            },
        ))
    }

    fn save_memory(&mut self, memory: &memory::Memory) -> Result<(), String> {
        let _lock = history_file_lock(&self.path)?;
        if read_history_generation(&self.path)? != self.generation {
            return Err(
                "Conversation history was reset in another window; the older worker will not overwrite it"
                    .into(),
            );
        }
        if memory::Memory::load(&self.path)? != self.expected {
            return Err(
                "Conversation history changed in another window; this worker will not overwrite it"
                    .into(),
            );
        }
        memory.save(&self.path)?;
        self.expected = memory.clone();
        Ok(())
    }
}

fn history_file_lock(path: &Path) -> Result<std::fs::File, String> {
    let lock_path = path.with_extension("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(|error| error.to_string())?;
    file.lock().map_err(|error| error.to_string())?;
    Ok(file)
}

fn read_history_generation(path: &Path) -> Result<u64, String> {
    let generation_path = path.with_extension("generation");
    let file = match std::fs::File::open(&generation_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.to_string()),
    };
    if file.metadata().map_err(|error| error.to_string())?.len() > HISTORY_GENERATION_BYTES {
        return Err("Conversation history generation is invalid".into());
    }
    let mut text = String::new();
    file.take(HISTORY_GENERATION_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| error.to_string())?;
    text.trim()
        .parse()
        .map_err(|_| "Conversation history generation is invalid".into())
}

/// Advance the tombstone and remove the older generation while the stable file lock is held.
fn advance_history_generation(path: &Path) -> Result<u64, String> {
    let next = read_history_generation(path)?
        .checked_add(1)
        .ok_or("Conversation history generation is exhausted")?;
    let generation_path = path.with_extension("generation");
    auris_session::settings::write_config_bytes(&generation_path, next.to_string().as_bytes())
        .map_err(|error| error.to_string())?;
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    Ok(next)
}

fn clear_history(path: &Path) -> Result<(), String> {
    let _lock = history_file_lock(path)?;
    advance_history_generation(path).map(drop)
}

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

    pub(super) fn open_history(
        &self,
        path: &Path,
        fresh: bool,
    ) -> Result<(memory::Memory, HistoryGeneration), String> {
        self.history(|| HistoryGeneration::load(path, fresh))
    }

    pub(super) fn save_history(
        &self,
        generation: &mut HistoryGeneration,
        memory: &memory::Memory,
    ) -> Result<(), String> {
        self.history(|| generation.save_memory(memory))
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
        history: Option<HistorySnapshot>,
    ) -> Result<Self, String> {
        let Command::Run(options) = parse_command(&[], &|name| std::env::var(name).ok(), &prefs)?
        else {
            return Err("Expected agent settings".into());
        };
        let history_path = folder
            .as_ref()
            .map(|folder| folder.join(".auris-conversation.json"));
        if let Some(snapshot) = history.as_ref()
            && history_path.as_deref() != Some(snapshot.path())
        {
            return Err("The conversation snapshot belongs to another project".into());
        }
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
                            json_conversation(&agent, &options, &bridge, history_path, fresh, history).await
                        } => result,
                    }
                });
                // Read-only reference work already running on blocking threads may finish,
                // but it has no session access and must not hold up cancellation.
                runtime.shutdown_timeout(std::time::Duration::from_millis(100));
                result
            })).unwrap_or_else(|_| Err("The agent worker panicked".into()));
            if let Err(message) = result { bridge.emit(serde_json::json!({"event":"error", "message":message})); }
            bridge.0.stopped.store(true, Ordering::Release);
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
        if self.stopped.load(Ordering::Acquire) {
            return Err("The agent worker stopped".into());
        }
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

/// Remove one persisted conversation after every older history operation has finished.
///
/// The wait happens on a short-lived worker thread. Once this reports success, a cancelled
/// conversation cannot finish a late atomic write and recreate the file.
pub fn clear_history_background(path: PathBuf) -> mpsc::Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    let failed = sender.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("auris-history-clear".into())
        .spawn(move || {
            let result = HISTORY_IO
                .lock()
                .map_err(|_| "Conversation storage failed".to_string())
                .and_then(|_guard| clear_history(&path));
            let _ = sender.send(result);
        })
    {
        let _ = failed.send(Err(error.to_string()));
    }
    receiver
}

/// Read one persisted conversation without constructing or contacting a model provider.
///
/// The desktop uses this when a project or its Agent panel opens, so the transcript is present
/// before a first message can be sent. The same process and file locks as the conversation worker
/// keep this read ordered with in-flight saves and clears.
pub fn load_history_background(
    path: PathBuf,
    fresh: bool,
) -> mpsc::Receiver<Result<HistorySnapshot, String>> {
    let (sender, receiver) = mpsc::channel();
    let failed = sender.clone();
    if let Err(error) = std::thread::Builder::new()
        .name("auris-history-load".into())
        .spawn(move || {
            let result = HISTORY_IO
                .lock()
                .map_err(|_| "Conversation storage failed".to_string())
                .and_then(|_guard| {
                    HistoryGeneration::load(&path, fresh)
                        .map(|(memory, generation)| HistorySnapshot { memory, generation })
                });
            let _ = sender.send(result);
        })
    {
        let _ = failed.send(Err(error.to_string()));
    }
    receiver
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
    fn history_clear_waits_for_an_in_flight_writer_and_removes_its_final_file() {
        let root = tempfile::tempdir().unwrap();
        let history = root.path().join(".auris-conversation.json");
        std::fs::write(&history, b"old history").unwrap();

        let writer = HISTORY_IO.lock().unwrap();
        let cleared = clear_history_background(history.clone());
        let early = cleared.try_recv();

        // Reproduce a cancelled worker finishing the atomic replacement it already began.
        let late_write = std::fs::write(&history, b"late worker history");
        drop(writer);
        assert_eq!(early, Err(mpsc::TryRecvError::Empty));
        late_write.unwrap();

        assert_eq!(
            cleared.recv_timeout(Duration::from_secs(2)).unwrap(),
            Ok(())
        );
        assert!(
            !history.exists(),
            "a cancelled worker's final write resurrected cleared history"
        );
    }

    #[test]
    fn history_load_reads_completed_turns_without_starting_a_provider() {
        let root = tempfile::tempdir().unwrap();
        let history = root.path().join(".auris-conversation.json");
        let mut memory = memory::Memory {
            summary: "Restored decisions, not a new instruction.".into(),
            ..Default::default()
        };
        memory.push("keep the intro", "The intro is unchanged.");
        memory.push("add a bass", "Added the bass track.");
        memory.save(&history).unwrap();

        let snapshot = load_history_background(history, false)
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();

        assert_eq!(
            snapshot.turns(),
            vec![
                ("keep the intro".into(), "The intro is unchanged.".into()),
                ("add a bass".into(), "Added the bass track.".into()),
            ]
        );
        assert_eq!(
            snapshot.summary(),
            Some("Restored decisions, not a new instruction.")
        );
    }

    #[test]
    fn history_file_lock_serializes_independent_handles() {
        let root = tempfile::tempdir().unwrap();
        let history = root.path().join(".auris-conversation.json");
        let first = history_file_lock(&history).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let other_history = history.clone();
        let waiter = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _second = history_file_lock(&other_history).unwrap();
            acquired_tx.send(()).unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(acquired_rx.try_recv(), Err(mpsc::TryRecvError::Empty));
        drop(first);
        acquired_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        waiter.join().unwrap();
        assert!(
            history.with_extension("lock").exists(),
            "the stable coordination file must survive every operation"
        );
    }

    #[test]
    fn stale_generation_cannot_recreate_cleared_history() {
        let root = tempfile::tempdir().unwrap();
        let history = root.path().join(".auris-conversation.json");
        let mut original = memory::Memory::default();
        original.push("old request", "old answer");
        original.save(&history).unwrap();
        let (_, mut stale) = HistoryGeneration::load(&history, false).unwrap();

        clear_history(&history).unwrap();
        let mut late = original;
        late.push("late request", "late answer");
        let error = stale.save_memory(&late).unwrap_err();

        assert!(error.contains("reset in another window"), "{error}");
        assert!(!history.exists(), "stale history was recreated after clear");
        assert!(history.with_extension("generation").exists());
        assert!(history.with_extension("lock").exists());
    }

    #[test]
    fn successful_history_saves_advance_the_expected_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let history = root.path().join(".auris-conversation.json");
        let (mut memory, mut generation) = HistoryGeneration::load(&history, false).unwrap();

        memory.push("first request", "first answer");
        generation.save_memory(&memory).unwrap();
        memory.push("second request", "second answer");
        generation.save_memory(&memory).unwrap();

        assert_eq!(memory::Memory::load(&history).unwrap(), memory);
    }

    #[test]
    fn displayed_snapshot_drives_the_worker_but_cannot_clobber_a_concurrent_append() {
        let root = tempfile::tempdir().unwrap();
        let history = root.path().join(".auris-conversation.json");
        let mut visible = memory::Memory {
            summary: "Visible restored context".into(),
            ..Default::default()
        };
        visible.push("visible request", "visible answer");
        visible.save(&history).unwrap();
        let snapshot = load_history_background(history.clone(), false)
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();

        let mut concurrent = visible.clone();
        concurrent.push("other window", "newer answer");
        concurrent.save(&history).unwrap();

        let done = completion(r#"{"role":"assistant","content":"local answer"}"#, "stop");
        let (url, requests) = mock_server(vec![done]);
        let worker = Worker::spawn(
            prefs(url),
            Some(root.path().to_path_buf()),
            false,
            Some(snapshot),
        )
        .unwrap();
        let first = worker
            .events
            .recv_timeout(Duration::from_secs(3))
            .expect("worker ready");
        assert_eq!(
            first["event"], "ready",
            "a preloaded worker must not replay or save history during startup"
        );
        assert_eq!(memory::Memory::load(&history).unwrap(), concurrent);

        worker
            .send(r#"{"say":"new visible request","display":"new visible request"}"#)
            .unwrap();
        let mut notice = None;
        loop {
            let event = worker
                .events
                .recv_timeout(Duration::from_secs(3))
                .expect("worker event");
            match event["event"].as_str().unwrap_or_default() {
                "notice" => notice = event["message"].as_str().map(str::to_string),
                "answer" => break,
                "error" | "ended" => panic!("{event}"),
                _ => {}
            }
        }

        let notice = notice.expect("the UI receives a nonfatal save-conflict notice");
        assert!(notice.contains("changed in another window"), "{notice}");
        assert_eq!(memory::Memory::load(&history).unwrap(), concurrent);
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("Visible restored context"));
        assert!(requests[0].contains("visible request"));
        assert!(requests[0].contains("visible answer"));
        assert!(!requests[0].contains("other window"));
        assert!(!requests[0].contains("newer answer"));
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
            let worker = Worker::spawn(preferences, None, false, None).unwrap();
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
        let worker = Worker::spawn(preferences, None, false, None).unwrap();
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
            None,
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
    fn invalid_track_kind_gets_actionable_feedback_and_recovers_through_rig() {
        let response = |kind: serde_json::Value| {
            completion(
            &serde_json::json!({"role":"assistant","tool_calls":[{"id":"track","type":"function","function":{"name":"add_track","arguments":serde_json::json!({"name":"Drums","kind":kind}).to_string()}}]}).to_string(),
            "tool_calls",
        )
        };
        let done = completion(
            r#"{"role":"assistant","content":"Added the drum track"}"#,
            "stop",
        );
        let (url, requests) = mock_server(vec![
            response(serde_json::json!(["drum"])),
            response(serde_json::json!("drum")),
            done,
        ]);
        let worker = Worker::spawn(prefs(url), None, false, None).unwrap();
        worker.send(r#"{"say":"Add a drum track"}"#).unwrap();
        let mut permissions = 0;
        let mut failures = 0;
        let mut edits = 0;
        let mut session = auris_session::Session::new(
            auris_session::SessionOptions::headless().with_balance(false),
        )
        .unwrap();
        loop {
            let event = worker
                .events
                .recv_timeout(Duration::from_secs(10))
                .expect("worker event");
            match event["event"].as_str().unwrap_or_default() {
                "permission" => {
                    permissions += 1;
                    assert_eq!(event["args"]["command"]["kind"], "drum");
                    worker.send(&serde_json::json!({"event":"permission_result","id":event["id"],"ok":true}).to_string()).unwrap();
                }
                "edit" => {
                    edits += 1;
                    let command = serde_json::from_value(event["command"].clone()).unwrap();
                    let text = session.agent_command(command).unwrap();
                    worker
                        .send(
                            &serde_json::json!({"event":"edit_result","ok":true,"text":text})
                                .to_string(),
                        )
                        .unwrap();
                }
                "result" if event["ok"] == false => {
                    failures += 1;
                    let error = event["text"].as_str().unwrap();
                    assert!(
                        error.contains("arguments.kind")
                            && error.contains("string")
                            && error.contains("drum"),
                        "{error}"
                    );
                }
                "answer" => break,
                "error" | "ended" => panic!("{event}"),
                _ => {}
            }
        }
        assert_eq!((failures, permissions, edits), (1, 1, 1));
        assert_eq!(session.project().tracks.len(), 1);
        assert!(session.path().is_none());
        let requests = requests.lock().unwrap();
        assert!(requests[1].contains("arguments.kind"));
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let tool = request["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|tool| tool["function"]["name"] == "add_track")
            .unwrap();
        assert_eq!(
            tool["function"]["parameters"]["properties"]["kind"]["type"],
            "string"
        );
        assert!(tool["function"]["parameters"]["properties"]["kind"]["enum"].is_array());
    }

    #[test]
    fn instrument_search_uses_the_permission_checked_live_session() {
        let call = completion(
            r#"{"role":"assistant","tool_calls":[{"id":"sounds","type":"function","function":{"name":"search_instruments","arguments":"{\"query\":\"strings\"}"}}]}"#,
            "tool_calls",
        );
        let done = completion(
            r#"{"role":"assistant","content":"Found the live library"}"#,
            "stop",
        );
        let (url, requests) = mock_server(vec![call, done]);
        let worker = Worker::spawn(prefs(url), None, false, None).unwrap();
        worker.send(r#"{"say":"Find string instruments"}"#).unwrap();
        let permission = until(&worker, "permission");
        let operation = auris_session::agent_policy::Operation::parse(
            permission["tool"].as_str().unwrap(),
            &permission["args"],
        )
        .unwrap();
        assert!(!operation.mutating);
        assert_eq!(operation.name, "search_instruments");
        worker
            .send(
                &serde_json::json!({"event":"permission_result","id":permission["id"],"ok":true})
                    .to_string(),
            )
            .unwrap();
        let edit = until(&worker, "edit");
        assert_eq!(edit["command"]["action"], "search_instruments");
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
        let first = Worker::spawn(prefs(first_url), None, false, None).unwrap();
        let second = Worker::spawn(prefs(second_url), None, false, None).unwrap();
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
        let mut worker = Worker::spawn(prefs(url), None, false, None).unwrap();
        worker.send(r#"{"say":"Create the lead"}"#).unwrap();
        until(&worker, "permission");
        worker.cancel();
        assert!(worker.send(r#"{"say":"too late"}"#).is_err());
        assert_eq!(
            worker.events.recv_timeout(Duration::from_secs(2)).unwrap()["event"],
            "ended"
        );
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
        let mut worker = Worker::spawn(prefs(url), None, false, None).unwrap();
        worker.send(r#"{"say":"hello"}"#).unwrap();
        connected.recv_timeout(Duration::from_secs(3)).unwrap();
        worker.cancel();
        until(&worker, "ended");
        let _ = release.send(());
        server.join().unwrap();
    }
}
