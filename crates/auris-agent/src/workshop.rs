//! Conversational song director and bounded, independently cancellable composers.

use auris_session::workshop::{
    Candidate, Direction, Document, Message, Preferences, Round, Selection, Store,
};
use auris_session::{AgentPreferences, Session, SessionOptions, Settings, SongCandidate, SongIdea};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

mod composer_log;
mod digest;
mod preview;
#[cfg(test)]
mod scenario_tests;

/// An immutable audition of an unfinished candidate, separate from adoptable music.
#[derive(Clone)]
pub struct Preview {
    /// Content revision captured before rendering.
    pub revision: u64,
    /// Rendered audio; never mutated while the user is listening.
    pub audio: Arc<auris_session::prelude::AudioBuffer>,
    /// Waveform and score sections captured for the same content revision.
    pub timeline: Arc<auris_session::AuditionTimeline>,
}

/// Automatic restarts allowed per candidate and explicit recovery request.
pub const AUTO_RECOVERY_LIMIT: u32 = 3;

/// Commands accepted by the background workshop owner.
pub enum Command {
    /// Create an independent song conversation. Work in the one being left carries on.
    NewThread,
    /// Open a conversation by its host-assigned identity. Work in the one being left carries on.
    OpenThread(String),
    /// Rename an exact conversation.
    RenameThread(String, String),
    /// Archive or restore an exact conversation without deleting its artifacts.
    ArchiveThread(String, bool),
    /// Permanently delete an exact conversation and its generated songs.
    DeleteThread(String),
    /// Record explicit human feedback for one finished candidate without starting model work.
    Feedback(String, u64, auris_session::workshop::Feedback),
    /// Submit one natural-language message with model settings captured at submission.
    Send(String, Box<AgentPreferences>, Vec<PathBuf>),
    /// [`Command::Send`] aimed at an exact conversation, which need not be the open one: a
    /// message queued in a conversation the user has left goes out once that one is idle.
    SendTo(String, String, Box<AgentPreferences>, Vec<PathBuf>),
    /// Choose an exact completed candidate without closing the conversation.
    Select(u64, Box<AgentPreferences>, String),
    /// Retry only this candidate in a new artifact directory.
    Retry(u64, Box<AgentPreferences>),
    /// Continue an unfinished candidate from its checkpoint with a fresh recovery budget.
    Recover(u64, Box<AgentPreferences>),
    /// Load a comparison's auditions alongside previously loaded rounds.
    View(usize),
    /// Stop one candidate, or all work when absent.
    Stop(Option<u64>),
    /// Persist a draft only if its captured conversation is still active.
    ThreadDraft(String, String, Vec<PathBuf>),
    /// Enable or disable cross-project preference evidence.
    Memory(bool),
    /// Refresh preferences edited in another application surface.
    ReloadPreferences,
    /// Forget all recorded preferences.
    Forget,
    /// Remove one incorrect or no-longer-relevant preference observation.
    ForgetChoice(String, u64),
}

/// Full UI snapshot, containing immutable shared audition buffers.
pub struct Snapshot {
    /// Project-local conversations available to the sidebar, including the archive.
    pub threads: Vec<auris_session::workshop::ThreadInfo>,
    /// Durable conversation and comparison history.
    pub document: Document,
    /// Completed candidates currently available for preview or adoption.
    pub candidates: BTreeMap<u64, Arc<SongCandidate>>,
    /// Latest in-progress auditions, which cannot be selected or saved as finished songs.
    pub previews: BTreeMap<u64, Preview>,
    /// Nonfatal audition failures; cleared after the next successful audition.
    pub preview_errors: BTreeMap<u64, String>,
    /// Real current stages; absence means the job is not running.
    pub progress: BTreeMap<u64, String>,
    /// The composer's own latest account of what it is doing, from its report_status calls.
    /// Shown only while its stage is the composer's; see [`Snapshot::status_line`].
    pub status: BTreeMap<u64, String>,
    /// The director is responding.
    pub planning: bool,
    /// Preference evidence the user may inspect or clear.
    pub preferences: Preferences,
    /// Storage/provider/runtime failure, never silently swallowed.
    pub error: Option<String>,
    /// Conversations whose director or composers are at work, the open one included. A
    /// conversation left while busy keeps working in the background until it is done.
    pub working: BTreeSet<String>,
    /// How many commands the owner had handled when this snapshot was taken, to compare with
    /// [`Workshop::sent`]: a snapshot that has caught up has seen the effect of every send.
    pub handled: u64,
}

/// One background owner; dropping it cancels all child workers.
pub struct Workshop {
    sender: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Snapshot>,
    stopped: Arc<AtomicBool>,
    sent: std::sync::atomic::AtomicU64,
}

impl Snapshot {
    /// The line a running candidate shows: the composer's own words while it is composing,
    /// `None` when the host's stage should be shown instead.
    ///
    /// Planning, rendering, stopping and recovery are the host's work, which the composer cannot
    /// see, so its last line would be stale there; those stages always show the host's label.
    pub fn status_line(&self, id: u64) -> Option<&str> {
        let composing = matches!(
            self.progress.get(&id).map(String::as_str),
            Some("harmony" | "melody" | "sing" | "compose" | "arrange")
        );
        composing
            .then(|| self.status.get(&id))
            .flatten()
            .map(String::as_str)
    }
}

impl Workshop {
    /// Restore the library's last conversation off the UI thread, normally from
    /// [`Store::library`]. When another window already has that conversation open, this window
    /// starts a new one rather than failing, so a second window still reaches the history.
    pub fn start(library: Store, settings: Settings) -> Result<Self, String> {
        let (sender, commands) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let cancelled = stopped.clone();
        std::thread::Builder::new()
            .name("auris-workshop".into())
            .spawn(move || {
                let result = (|| {
                    let mut store = library.active_thread()?;
                    let lease = match store.lease() {
                        Ok(lease) => lease,
                        Err(_) => {
                            store = library.create_thread()?;
                            store.lease()?
                        }
                    };
                    // Do not overwrite an unreadable history with a fresh default.
                    store.load()?;
                    let mut owner = Owner::new(store, settings, events.clone());
                    owner.lease = Some(lease);
                    owner.run(commands, cancelled);
                    Ok::<(), String>(())
                })();
                if let Err(error) = result {
                    let _ = events.send(Snapshot {
                        threads: Vec::new(),
                        document: Document::default(),
                        candidates: BTreeMap::new(),
                        previews: BTreeMap::new(),
                        preview_errors: BTreeMap::new(),
                        progress: BTreeMap::new(),
                        status: BTreeMap::new(),
                        planning: false,
                        preferences: Preferences::default(),
                        error: Some(error),
                        working: BTreeSet::new(),
                        handled: 0,
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            receiver,
            stopped,
            sent: Default::default(),
        })
    }
    /// Queue a command without blocking the window.
    pub fn send(&self, command: Command) -> bool {
        let sent = self.sender.send(command).is_ok();
        if sent {
            self.sent.fetch_add(1, Ordering::AcqRel);
        }
        sent
    }
    /// How many commands this window has handed over; see [`Snapshot::handled`].
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Acquire)
    }
    /// Drain available snapshots; callers should keep the newest.
    pub fn poll(&self) -> Option<Snapshot> {
        self.receiver.try_iter().last()
    }
    /// A workshop with no owner thread: the caller receives its commands and supplies its
    /// snapshots. For a host's tests, which need to see what the window sent and to answer it
    /// without a model behind it.
    pub fn loopback() -> (Self, mpsc::Receiver<Command>, mpsc::Sender<Snapshot>) {
        let (sender, commands) = mpsc::channel();
        let (snapshots, receiver) = mpsc::channel();
        let workshop = Self {
            sender,
            receiver,
            stopped: Arc::new(AtomicBool::new(false)),
            sent: Default::default(),
        };
        (workshop, commands, snapshots)
    }
}

impl Drop for Workshop {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
    }
}

/// Create an editor conversation with the workshop's readable history and bounded model memory.
/// Run on a storage worker; the returned store and ID are ready for normal conversation navigation.
pub fn handoff(
    folder: PathBuf,
    unsaved: bool,
    library: Store,
    document: Document,
) -> Result<(auris_session::conversations::ConversationStore, String), String> {
    use auris_session::conversations::{
        ConversationContent, ConversationMessage, ConversationStore,
    };
    let store = if unsaved {
        ConversationStore::for_unsaved_document(&folder)?
    } else {
        ConversationStore::for_project(&folder)
    };
    let mut conversation = store.create()?;
    let mut memory = crate::memory::Memory::default();
    let mut pending = String::new();
    for (index, message) in document.messages.iter().enumerate() {
        conversation.transcript.messages.push(ConversationMessage {
            id: index as u64,
            model: String::new(),
            content: if message.user {
                if !pending.is_empty() {
                    pending.push('\n');
                }
                pending.push_str(&message.text);
                for reference in &message.references {
                    pending.push('\n');
                    pending.push_str(&reference.text);
                }
                ConversationContent::User {
                    text: message.text.clone(),
                    attachments: store.capture_attachments(
                        &conversation.info.id,
                        &library
                            .thread(if document.thread_id.is_empty() {
                                "default"
                            } else {
                                &document.thread_id
                            })?
                            .owned_attachments(&message.attachments),
                    )?,
                }
            } else {
                memory.push(&pending, &message.text);
                pending.clear();
                ConversationContent::Assistant {
                    text: message.text.clone(),
                }
            },
        });
    }
    if !pending.is_empty() {
        memory.push(&pending, "");
    }
    if let Some(selected) = document
        .rounds
        .iter()
        .flat_map(|r| &r.candidates)
        .find(|c| Some(c.id) == document.selected)
    {
        memory.summary = format!(
            "The user selected this exact song and opened it in the editor. Continue against the current score; do not recreate it. Selection: {}. {}. Assignment: {}",
            selected.direction.title,
            selected.direction.description,
            selected.direction.instructions
        );
    }
    let feedback = feedback_context(&document);
    if !feedback.is_empty() {
        memory.summary.push_str("\nUser-authored song feedback: ");
        memory.summary.push_str(&json!(feedback).to_string());
    }
    conversation.transcript.draft = document.draft;
    conversation.transcript.attachments = document.draft_attachments;
    store.save(&conversation.info.id, &conversation.transcript)?;
    memory.save(&store.memory_path(&conversation.info.id)?)?;
    Ok((store, conversation.info.id))
}

/// Decode explicitly attached symbolic references once, on the background owner.
fn capture_references(
    paths: &[PathBuf],
) -> Result<Vec<auris_session::workshop::Reference>, String> {
    if paths.len() > crate::ATTACHMENT_COUNT_LIMIT {
        return Err("Too many music references".into());
    }
    let paths: Vec<_> = paths
        .iter()
        .filter(|path| {
            matches!(
                crate::attachment_kind(path),
                Some(crate::AttachmentKind::Midi | crate::AttachmentKind::Text)
            )
        })
        .collect();
    let names: Vec<_> = paths
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let decoded = crate::attachments::read_references(
        &names,
        crate::ATTACHMENT_TOTAL_CEILING,
        crate::ATTACHMENT_COUNT_LIMIT,
    )?;
    paths
        .iter()
        .zip(decoded)
        .map(|(path, reference)| match reference {
            crate::attachments::Reference::Text(text) => Ok(auris_session::workshop::Reference {
                path: (*path).clone(),
                text,
                snapshot: None,
            }),
            _ => Err("Expected a symbolic music reference".into()),
        })
        .collect()
}

fn composition_assignment(candidate: &Candidate, document: &Document) -> String {
    json!({"assignment":candidate.direction.instructions,
        "conversation_summary":conversation_summary(document),
        "conversation":document.model_messages(),
        "user_feedback":feedback_context(document)})
    .to_string()
}

/// The summary standing in for messages no longer passed verbatim, or null when there is none.
fn conversation_summary(document: &Document) -> serde_json::Value {
    if document.digest.is_empty() {
        serde_json::Value::Null
    } else {
        json!(document.digest)
    }
}

fn feedback_context(document: &Document) -> Vec<serde_json::Value> {
    document
        .rounds
        .iter()
        .rev()
        .flat_map(|r| r.candidates.iter().rev())
        .filter_map(|c| {
            c.feedback.as_ref().map(|feedback| {
                json!({
                    "candidate":c.id, "title":c.direction.title,
                    "description":c.direction.description, "rating":feedback.rating,
                    "comment":feedback.comment, "source":"user"
                })
            })
        })
        .take(16)
        .collect()
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    reply: String,
    directions: Vec<Direction>,
    #[serde(default)]
    choices: Option<auris_session::choices::ChoiceCard>,
}

fn decision(text: &str) -> Result<Decision, String> {
    if text.len() > 100_000 {
        return Err("Director response is too large".into());
    }
    let text = text.trim();
    let text = text
        .strip_prefix("```json")
        .or_else(|| text.strip_prefix("```"))
        .and_then(|s| s.strip_suffix("```"))
        .unwrap_or(text)
        .trim();
    let value: Decision =
        serde_json::from_str(text).map_err(|e| format!("Invalid director response: {e}"))?;
    if value.reply.trim().is_empty()
        || value.reply.chars().count() > 4000
        || value.directions.len() > 3
        || value.directions.iter().any(|d| {
            d.title.trim().is_empty()
                || d.title.chars().count() > 80
                || d.description.chars().count() > 400
                || d.instructions.trim().is_empty()
                || d.instructions.chars().count() > 16000
        })
    {
        return Err("Invalid or oversized composition assignments".into());
    }
    if let Some(choices) = &value.choices {
        choices
            .validate()
            .map_err(|e| format!("Invalid director choices: {e}"))?;
    }
    Ok(value)
}

/// Lift the first run of two to eight numbered lines out of a reply into a multiple-choice card.
///
/// Returns the reply without those lines, and the card; the question is the reply's last
/// remaining line that asks one, or else its last line. `None` when there is no such list, or
/// the list would not make a card a person could answer.
fn numbered_choices(reply: &str) -> Option<(String, auris_session::choices::ChoiceCard)> {
    use auris_session::choices::{ChoiceCard, ChoiceOption, MAX_OPTIONS};
    fn item(line: &str) -> Option<&str> {
        let line = line.trim_start();
        let digits = line
            .char_indices()
            .take_while(|(_, c)| c.is_ascii_digit() || ('０'..='９').contains(c))
            .last()
            .map(|(at, c)| at + c.len_utf8())?;
        let rest = &line[digits..];
        let mark = rest.chars().next()?;
        ['.', ')', '．', '）', '、']
            .contains(&mark)
            .then(|| rest[mark.len_utf8()..].trim())
            .filter(|text| !text.is_empty())
    }
    fn clip(text: &str, limit: usize) -> String {
        text.chars()
            .take(limit)
            .collect::<String>()
            .trim()
            .to_owned()
    }
    let lines: Vec<&str> = reply.lines().collect();
    let start = lines.iter().position(|line| item(line).is_some())?;
    let end = start
        + lines[start..]
            .iter()
            .take_while(|line| item(line).is_some())
            .count();
    if !(2..=MAX_OPTIONS).contains(&(end - start)) {
        return None;
    }
    let options = lines[start..end]
        .iter()
        .filter_map(|line| item(line))
        .map(|text| {
            let text = text.replace("**", "");
            let split = ["：", ": ", " — ", " - "]
                .iter()
                .filter_map(|separator| text.find(separator).map(|at| (at, separator.len())))
                .min();
            let (label, detail) = match split {
                Some((at, width)) if at > 0 => (&text[..at], Some(text[at + width..].trim())),
                _ => (text.as_str(), None),
            };
            ChoiceOption {
                label: clip(label, 80),
                detail: detail.filter(|d| !d.is_empty()).map(|d| clip(d, 200)),
            }
        })
        .collect();
    let rest: Vec<&str> = lines[..start]
        .iter()
        .chain(&lines[end..])
        .copied()
        .collect();
    let spoken: Vec<&str> = rest
        .iter()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty())
        .collect();
    let question = spoken
        .iter()
        .rev()
        .find(|line| line.ends_with('?') || line.ends_with('？'))
        .or(spoken.last())?;
    let card = ChoiceCard {
        question: clip(question, 200),
        options,
        multiple: true,
    };
    card.validate().ok()?;
    Some((rest.join("\n").trim().to_owned(), card))
}

enum JobEvent {
    Tool(u64, String, Option<bool>),
    Progress(u64, String),
    Status(u64, String),
    Preview(u64, Result<Preview, String>),
    Recovering(u64, u32, String),
    Done(u64, Result<Arc<SongCandidate>, String>),
}

// Backends can repeat call notifications while streaming. Count each invocation once,
// and keep IDs local to an attempt because a restarted backend may reuse them.
fn observe_tool_event(
    candidate: u64,
    event: &serde_json::Value,
    calls: &mut BTreeMap<String, bool>,
    sender: &mpsc::Sender<JobEvent>,
) {
    let kind = event["event"].as_str().unwrap_or_default();
    if !matches!(kind, "call" | "result") {
        return;
    }
    let name = event["tool"].as_str().unwrap_or_default();
    if name.is_empty() {
        return;
    }
    let id = event["call_id"].as_str().unwrap_or(name).to_owned();
    if let std::collections::btree_map::Entry::Vacant(entry) = calls.entry(id.clone()) {
        entry.insert(false);
        let _ = sender.send(JobEvent::Tool(candidate, name.into(), None));
    }
    if kind == "result" && calls.get(&id) == Some(&false) {
        calls.insert(id, true);
        let _ = sender.send(JobEvent::Tool(
            candidate,
            name.into(),
            Some(event["ok"] == true),
        ));
    }
}
struct Owner {
    lease: Option<std::fs::File>,
    store: Store,
    settings: Settings,
    snapshot: Snapshot,
    events: mpsc::Sender<Snapshot>,
    planner: Option<crate::song_ideas::SongPlanner>,
    conversation_digest: Option<digest::ConversationDigest>,
    preference_digest: Option<digest::PreferenceDigest>,
    /// The furthest summary that failed, so it is not retried until it would cover more.
    conversation_digest_failed: usize,
    preference_digest_failed: usize,
    planned_parent: Option<u64>,
    selection_followup: bool,
    prefs: AgentPreferences,
    jobs: BTreeMap<u64, Arc<AtomicBool>>,
    job_sender: mpsc::Sender<JobEvent>,
    job_receiver: mpsc::Receiver<JobEvent>,
    handles: Vec<std::thread::JoinHandle<()>>,
    /// Conversations left while their director or composers were still at work, by thread id.
    /// Each keeps its own lease and workers, and is let go once it is idle. Only the open
    /// conversation's map is used; a parked owner's own is always empty.
    parked: BTreeMap<String, Owner>,
    /// Set on a parked owner: it saves what happens, but the window is not looking at it.
    is_parked: bool,
    /// A parked owner saved a change the sidebar may want to show.
    parked_changed: bool,
    /// Commands handled by this window's owner so far; carried across conversation changes.
    handled: u64,
}

impl Drop for Owner {
    /// Cancel and wait for this conversation's workers, so its lease and files are free after.
    fn drop(&mut self) {
        for cancel in self.jobs.values() {
            cancel.store(true, Ordering::Release);
        }
        self.planner = None;
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

impl Owner {
    fn new(store: Store, settings: Settings, events: mpsc::Sender<Snapshot>) -> Self {
        let (job_sender, job_receiver) = mpsc::channel();
        let mut error = None;
        let mut document = store.load().unwrap_or_else(|e| {
            error = Some(e);
            Document::default()
        });
        let threads = store.threads().unwrap_or_else(|e| {
            error.get_or_insert(e);
            Vec::new()
        });
        for candidate in document.rounds.iter_mut().flat_map(|r| &mut r.candidates) {
            for usage in candidate.tools.values_mut() {
                usage.interrupt();
            }
            if !candidate.ready && candidate.error.is_none() {
                candidate.error = Some("Production was interrupted; retry this candidate".into());
            }
        }
        let preferences = auris_session::workshop::load_preferences().unwrap_or_else(|e| {
            error = Some(e);
            Preferences::default()
        });
        let mut candidates = BTreeMap::new();
        let baseline = document.continuation_candidate().map(|c| c.id);
        for candidate in document
            .rounds
            .iter()
            .flat_map(|r| &r.candidates)
            .filter(|c| {
                c.ready
                    && (Some(c.id) == baseline
                        || document
                            .rounds
                            .last()
                            .is_some_and(|r| r.candidates.iter().any(|last| last.id == c.id)))
            })
        {
            match SongCandidate::load_workshop(
                &store.candidate_path(candidate.id),
                idea(&candidate.direction),
            ) {
                Ok(song) => {
                    candidates.insert(candidate.id, Arc::new(song));
                }
                Err(e) => error = Some(e),
            }
        }
        Self {
            lease: None,
            prefs: settings.agent.clone(),
            store,
            settings,
            events,
            planner: None,
            conversation_digest: None,
            preference_digest: None,
            conversation_digest_failed: 0,
            preference_digest_failed: 0,
            planned_parent: None,
            selection_followup: false,
            jobs: BTreeMap::new(),
            job_sender,
            job_receiver,
            handles: Vec::new(),
            parked: BTreeMap::new(),
            is_parked: false,
            parked_changed: false,
            handled: 0,
            snapshot: Snapshot {
                threads,
                document,
                candidates,
                previews: BTreeMap::new(),
                preview_errors: BTreeMap::new(),
                progress: BTreeMap::new(),
                status: BTreeMap::new(),
                planning: false,
                preferences,
                error,
                working: BTreeSet::new(),
                handled: 0,
            },
        }
    }
    fn thread_id(&self) -> &str {
        &self.snapshot.document.thread_id
    }
    /// Whether the director or a composer of this conversation is at work.
    fn busy(&self) -> bool {
        self.planner.is_some() || !self.jobs.is_empty()
    }
    fn publish(&mut self, save: bool) {
        if save && let Err(e) = self.store.save(&self.snapshot.document) {
            self.snapshot.error = Some(e);
        }
        if self.is_parked {
            self.parked_changed |= save;
            return;
        }
        let shown = std::iter::once(&self.snapshot.document)
            .chain(self.parked.values().map(|owner| &owner.snapshot.document));
        for document in shown {
            if let Some(thread) = self
                .snapshot
                .threads
                .iter_mut()
                .find(|t| t.id == document.thread_id)
            {
                thread.title = document.sidebar_title();
                thread.archived = document.archived;
            }
        }
        let mut working: BTreeSet<String> = self.parked.keys().cloned().collect();
        if self.busy() {
            working.insert(self.thread_id().to_owned());
        }
        let _ = self.events.send(Snapshot {
            threads: self.snapshot.threads.clone(),
            document: self.snapshot.document.clone(),
            candidates: self.snapshot.candidates.clone(),
            previews: self.snapshot.previews.clone(),
            preview_errors: self.snapshot.preview_errors.clone(),
            progress: self.snapshot.progress.clone(),
            status: self.snapshot.status.clone(),
            planning: self.planner.is_some(),
            preferences: self.snapshot.preferences.clone(),
            error: self.snapshot.error.clone(),
            working,
            handled: self.handled,
        });
    }
    fn run(&mut self, commands: mpsc::Receiver<Command>, stopped: Arc<AtomicBool>) {
        self.publish(false);
        while !stopped.load(Ordering::Acquire) {
            match commands.recv_timeout(Duration::from_millis(25)) {
                Ok(command) => {
                    self.handled += 1;
                    self.command(command);
                    self.publish(true);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.tick();
            self.digest();
            self.tick_parked();
        }
        self.halt();
        // Persist accepted storage-only commands even when the window closes immediately.
        for command in commands.try_iter() {
            if matches!(
                command,
                Command::ThreadDraft(..)
                    | Command::Feedback(..)
                    | Command::NewThread
                    | Command::OpenThread(_)
                    | Command::RenameThread(..)
                    | Command::ArchiveThread(..)
                    | Command::DeleteThread(_)
            ) {
                self.command(command);
            }
        }
        for mut owner in std::mem::take(&mut self.parked).into_values() {
            owner.halt();
            owner.publish(true);
        }
        self.publish(true);
    }
    /// Cancel this conversation's work and wait for its workers to stop.
    fn halt(&mut self) {
        for cancel in self.jobs.values() {
            cancel.store(true, Ordering::Release);
        }
        self.jobs.clear();
        self.snapshot.progress.clear();
        self.planner = None;
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        for usage in self
            .snapshot
            .document
            .rounds
            .iter_mut()
            .flat_map(|r| &mut r.candidates)
            .flat_map(|c| c.tools.values_mut())
        {
            usage.interrupt();
        }
    }
    /// Advance the conversations left at work, and let go of the ones that have finished.
    fn tick_parked(&mut self) {
        if self.parked.is_empty() {
            return;
        }
        let mut changed = false;
        self.parked.retain(|_, owner| {
            owner.tick();
            changed |= std::mem::take(&mut owner.parked_changed);
            let busy = owner.busy();
            changed |= !busy;
            busy
        });
        if changed {
            self.publish(false);
        }
    }
    /// Take in the director's answer and the composers' events for this conversation.
    fn tick(&mut self) {
        {
            if let Some(answer) = self.planner.as_mut().and_then(|p| p.poll_text()) {
                self.planner = None;
                match answer.and_then(|text| decision(&text)) {
                    Err(e) => self.snapshot.error = Some(e),
                    Ok(mut answer) => {
                        // After "Continue with this song" the suggestions are always a card,
                        // even from a model that wrote them as a numbered list anyway.
                        if self.selection_followup
                            && answer.choices.is_none()
                            && let Some((reply, card)) = numbered_choices(&answer.reply)
                        {
                            answer.reply = reply;
                            answer.choices = Some(card);
                        }
                        self.snapshot.document.messages.push(Message {
                            choices: answer.choices,
                            references: vec![],
                            attachments: Vec::new(),
                            user: false,
                            text: answer.reply,
                        });
                        if !self.selection_followup && !answer.directions.is_empty() {
                            let mut candidates = Vec::new();
                            for mut direction in answer.directions {
                                direction.sources =
                                    usable_sources(&direction.sources, self.planned_parent, |id| {
                                        self.snapshot.candidates.contains_key(&id)
                                    });
                                self.snapshot.document.next_id += 1;
                                candidates.push(Candidate {
                                    id: self.snapshot.document.next_id,
                                    parent: self.planned_parent,
                                    direction,
                                    ready: false,
                                    error: None,
                                    recoveries: 0,
                                    tools: Default::default(),
                                    feedback: None,
                                });
                            }
                            self.snapshot.document.rounds.push(Round {
                                after_message: self.snapshot.document.messages.len(),
                                candidates: candidates.clone(),
                            });
                            for candidate in candidates {
                                self.launch(candidate);
                            }
                        }
                    }
                }
                self.publish(true);
            }
            while let Ok(event) = self.job_receiver.try_recv() {
                match event {
                    JobEvent::Tool(id, name, result) if self.jobs.contains_key(&id) => {
                        if let Some(candidate) = self
                            .snapshot
                            .document
                            .rounds
                            .iter_mut()
                            .flat_map(|r| &mut r.candidates)
                            .find(|c| c.id == id)
                        {
                            let usage = candidate.tools.entry(name).or_default();
                            match result {
                                None => usage.total += 1,
                                Some(true) => usage.succeeded += 1,
                                Some(false) => usage.failed += 1,
                            }
                        }
                        self.publish(true);
                    }
                    JobEvent::Preview(id, result)
                        if self
                            .jobs
                            .get(&id)
                            .is_some_and(|c| !c.load(Ordering::Acquire)) =>
                    {
                        match result {
                            Ok(preview) => {
                                self.snapshot.preview_errors.remove(&id);
                                self.snapshot.previews.insert(id, preview);
                            }
                            Err(error) => {
                                self.snapshot.preview_errors.insert(id, error);
                            }
                        }
                        self.publish(false);
                    }
                    JobEvent::Recovering(id, count, error) if self.jobs.contains_key(&id) => {
                        if let Some(candidate) = self
                            .snapshot
                            .document
                            .rounds
                            .iter_mut()
                            .flat_map(|r| &mut r.candidates)
                            .find(|c| c.id == id)
                        {
                            candidate.recoveries = count;
                            for usage in candidate.tools.values_mut() {
                                usage.interrupt();
                            }
                            candidate.error = Some(error);
                        }
                        self.snapshot.progress.insert(id, "recovering".into());
                        // A new attempt has not said what it is doing yet.
                        self.snapshot.status.remove(&id);
                        self.publish(true);
                    }
                    JobEvent::Progress(id, stage) if self.jobs.contains_key(&id) => {
                        if stage == "planning" {
                            self.snapshot.status.remove(&id);
                        }
                        self.snapshot.progress.insert(id, stage);
                        self.publish(false);
                    }
                    JobEvent::Status(id, status) if self.jobs.contains_key(&id) => {
                        self.snapshot.status.insert(id, status);
                        self.publish(false);
                    }
                    JobEvent::Done(id, result) => {
                        let cancelled = self
                            .jobs
                            .remove(&id)
                            .is_none_or(|c| c.load(Ordering::Acquire));
                        self.snapshot.progress.remove(&id);
                        self.snapshot.status.remove(&id);
                        // A single completed edit advances the selected song. Comparisons,
                        // older recovery jobs and a selection changed during production do not.
                        let advance_selection = !cancelled
                            && result.is_ok()
                            && self.snapshot.document.selected.is_some()
                            && self.snapshot.document.rounds.last().is_some_and(|round| {
                                matches!(round.candidates.as_slice(), [candidate]
                                    if candidate.id == id
                                        && candidate.parent == self.snapshot.document.selected)
                            });
                        {
                            let result = if cancelled {
                                Err("Composition stopped".into())
                            } else {
                                result
                            };
                            let entry = self
                                .snapshot
                                .document
                                .rounds
                                .iter_mut()
                                .flat_map(|r| &mut r.candidates)
                                .find(|c| c.id == id);
                            if let Some(entry) = entry {
                                for usage in entry.tools.values_mut() {
                                    usage.interrupt();
                                }
                                match result {
                                    Ok(candidate) => {
                                        entry.ready = true;
                                        entry.error = None;
                                        self.snapshot.previews.remove(&id);
                                        self.snapshot.preview_errors.remove(&id);
                                        self.snapshot.candidates.insert(id, candidate);
                                    }
                                    Err(e) => entry.error = Some(e),
                                }
                            }
                        }
                        if advance_selection {
                            self.snapshot.document.selected = Some(id);
                        }
                        self.publish(true);
                    }
                    _ => {}
                }
            }
        }
    }
    /// The conversation a command is addressed to, when it names one.
    fn addressee(command: &Command) -> Option<&str> {
        match command {
            Command::ThreadDraft(thread, ..)
            | Command::Feedback(thread, ..)
            | Command::SendTo(thread, ..) => Some(thread),
            _ => None,
        }
    }
    fn command(&mut self, command: Command) {
        if let Some(thread) = Self::addressee(&command)
            && thread != self.thread_id()
            && !self.is_parked
        {
            let thread = thread.to_owned();
            if matches!(command, Command::SendTo(..))
                && !self.parked.contains_key(&thread)
                && let Err(error) = self.park(&thread)
            {
                self.snapshot.error = Some(error);
                return;
            }
            if let Some(owner) = self.parked.get_mut(&thread) {
                owner.command(command);
                owner.publish(true);
                return;
            }
        }
        let command = match command {
            Command::SendTo(_, text, prefs, attachments) => Command::Send(text, prefs, attachments),
            command => command,
        };
        match command {
            Command::NewThread => {
                if let Err(error) = self.change_thread(None) {
                    self.snapshot.error = Some(error);
                }
            }
            Command::OpenThread(id) => {
                if id == self.snapshot.document.thread_id {
                    return;
                }
                if let Err(error) = self.change_thread(Some(&id)) {
                    self.snapshot.error = Some(error);
                }
            }
            Command::RenameThread(id, title) => {
                if let Err(error) = self.update_thread(&id, |document| document.rename(&title)) {
                    self.snapshot.error = Some(error);
                }
            }
            Command::ArchiveThread(id, archived) => {
                if let Err(error) = self.update_thread(&id, |document| {
                    document.archived = archived;
                    Ok(())
                }) {
                    self.snapshot.error = Some(error);
                    return;
                }
                if archived
                    && id == self.snapshot.document.thread_id
                    && let Err(error) = self.leave(&id)
                {
                    self.snapshot.error = Some(error);
                }
            }
            Command::DeleteThread(id) => {
                if id == self.snapshot.document.thread_id
                    && let Err(error) = self.leave(&id)
                {
                    self.snapshot.error = Some(error);
                    return;
                }
                // Deleting a conversation stops its work first: its workers write into it.
                drop(self.parked.remove(&id));
                let deleted = self
                    .store
                    .delete_thread(&id)
                    .and_then(|()| self.store.threads());
                match deleted {
                    Ok(threads) => self.snapshot.threads = threads,
                    Err(error) => self.snapshot.error = Some(error),
                }
            }
            Command::ThreadDraft(thread, text, attachments) => {
                if thread == self.snapshot.document.thread_id {
                    self.snapshot.document.draft = text;
                    self.snapshot.document.draft_attachments = attachments;
                }
            }
            Command::Feedback(thread, id, feedback) => {
                if thread != self.snapshot.document.thread_id {
                    self.snapshot.error = Some("Feedback belongs to another conversation".into());
                    return;
                }
                if let Err(error) = self.snapshot.document.record_feedback(id, feedback) {
                    self.snapshot.error = Some(error);
                }
            }
            Command::Send(text, prefs, attachments)
                if self.planner.is_none() && self.jobs.is_empty() =>
            {
                if text.trim().is_empty() || text.chars().count() > 16000 {
                    self.snapshot.error = Some("Enter 1..16000 characters".into());
                    return;
                }
                self.snapshot.document.draft = text.clone();
                self.snapshot.document.draft_attachments = attachments.clone();
                self.prefs = *prefs;
                let attachments = match crate::validate_attachments(&self.prefs, &attachments)
                    .and_then(|()| self.store.capture_attachments(&attachments))
                {
                    Ok(paths) => paths,
                    Err(error) => {
                        self.snapshot.error = Some(error);
                        return;
                    }
                };
                let references = match capture_references(&attachments) {
                    Ok(references) => references,
                    Err(error) => {
                        self.snapshot.error = Some(error);
                        return;
                    }
                };
                self.selection_followup = false;
                self.planned_parent = self
                    .snapshot
                    .document
                    .continuation_candidate()
                    .map(|c| c.id);
                self.snapshot.error = None;
                match auris_session::workshop::load_preferences() {
                    Ok(preferences) => self.snapshot.preferences = preferences,
                    Err(e) => {
                        self.snapshot.error = Some(e);
                        return;
                    }
                }
                self.snapshot.document.draft.clear();
                self.snapshot.document.draft_attachments.clear();
                self.snapshot.document.messages.push(Message {
                    choices: None,
                    user: true,
                    text,
                    references,
                    attachments,
                });
                let selected = self.snapshot.document.continuation_candidate();
                let voices = auris_session::library::voices_with_settings(&self.settings);
                let candidate_history: Vec<_> = self.snapshot.document.rounds.iter().enumerate().map(|(round_index, round)| {
                    json!({"after_message": round.after_message, "candidates": round.candidates.iter().enumerate().map(|(position, candidate)| {
                        json!({"id": candidate.id, "label": auris_session::workshop::candidate_label(round_index, position), "parent": candidate.parent,
                            "title": candidate.direction.title, "description": candidate.direction.description,
                            "feedback": candidate.feedback})
                    }).collect::<Vec<_>>()})
                }).collect();
                let prompt = json!({"conversation_summary": conversation_summary(&self.snapshot.document),
                    "conversation": self.snapshot.document.model_messages(), "selected": selected, "selected_score":self.planned_parent.and_then(|id| self.snapshot.candidates.get(&id)).map(|c| c.workshop_outline()),
                    "candidate_history": candidate_history,
                    "preference_evidence": self.snapshot.preferences.context(), "installed_voices": voices}).to_string();
                let references = self
                    .snapshot
                    .document
                    .messages
                    .iter()
                    .rev()
                    .find(|message| !message.attachments.is_empty())
                    .map(|message| message.attachments.as_slice())
                    .unwrap_or_default();
                let references = self.store.owned_attachments(references);
                match crate::song_ideas::SongPlanner::conversation_with_attachments(
                    self.prefs.clone(),
                    prompt,
                    &references,
                ) {
                    Ok(planner) => self.planner = Some(planner),
                    Err(e) => self.snapshot.error = Some(e),
                }
            }
            Command::Select(id, prefs, message) if self.snapshot.candidates.contains_key(&id) => {
                if self.selection_followup
                    && self.planner.is_some()
                    && self.snapshot.document.selected == Some(id)
                {
                    return;
                }
                self.planner = None;
                self.snapshot.error = None;
                self.snapshot.document.selected = Some(id);
                if let Some(round) = self
                    .snapshot
                    .document
                    .rounds
                    .iter()
                    .find(|r| r.candidates.iter().any(|c| c.id == id))
                {
                    let chosen = round
                        .candidates
                        .iter()
                        .position(|c| c.id == id)
                        .unwrap_or(0);
                    let selection = Selection {
                        conversation: self.store.identity(),
                        candidate: id,
                        round: round.candidates.first().map_or(id, |c| c.id),
                        context: self
                            .snapshot
                            .document
                            .messages
                            .iter()
                            .take(round.after_message)
                            .filter(|m| m.user)
                            .map(|m| m.text.as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                            .chars()
                            .take(2000)
                            .collect(),
                        alternatives: round
                            .candidates
                            .iter()
                            .map(|c| c.direction.clone())
                            .collect(),
                        chosen,
                    };
                    match auris_session::workshop::update_preferences(|p| p.record(selection)) {
                        Ok(p) => self.snapshot.preferences = p,
                        Err(e) => self.snapshot.error = Some(e),
                    }
                }
                self.prefs = *prefs;
                self.selection_followup = true;
                self.planned_parent = Some(id);
                self.snapshot.document.messages.push(Message {
                    choices: None,
                    references: vec![],
                    attachments: Vec::new(),
                    user: true,
                    text: message,
                });
                let selected = self
                    .snapshot
                    .document
                    .rounds
                    .iter()
                    .flat_map(|r| &r.candidates)
                    .find(|c| c.id == id);
                let recorded = self.snapshot.preferences.enabled && self.snapshot.error.is_none();
                let prompt = json!({
                    "event":"candidate_selected",
                    "conversation_summary":conversation_summary(&self.snapshot.document),
                    "conversation":self.snapshot.document.model_messages(),
                    "selected":selected,
                    "selected_score":self.snapshot.candidates[&id].workshop_outline(),
                    "user_feedback":feedback_context(&self.snapshot.document),
                    "preference_recorded":recorded,
                    "instruction":"Acknowledge the selected song and propose 3 to 5 concrete, distinct arrangement or improvement options as a choices card with multiple true, so several can be combined, grounded in this song and the conversation. Ask which to pursue in its question and invite a custom request in reply; do not list the options in reply. Mention preference recording only if preference_recorded is true. Return an empty directions array; selection requests suggestions, not another production batch. Do not claim to have listened."
                }).to_string();
                match crate::song_ideas::SongPlanner::conversation(self.prefs.clone(), prompt) {
                    Ok(planner) => self.planner = Some(planner),
                    Err(e) => self.snapshot.error = Some(e),
                }
            }
            Command::Recover(id, prefs)
                if self.jobs.len() < 3
                    && !self.jobs.contains_key(&id)
                    && self.planner.is_none() =>
            {
                let candidate = self
                    .snapshot
                    .document
                    .rounds
                    .iter_mut()
                    .flat_map(|r| &mut r.candidates)
                    .find(|c| c.id == id && !c.ready);
                if let Some(candidate) = candidate {
                    candidate.recoveries = 0;
                    let candidate = candidate.clone();
                    self.prefs = *prefs;
                    self.launch(candidate);
                }
            }
            Command::Retry(id, prefs)
                if self.jobs.len() < 3
                    && !self.jobs.contains_key(&id)
                    && self.planner.is_none() =>
            {
                let candidate = self
                    .snapshot
                    .document
                    .rounds
                    .iter()
                    .flat_map(|r| &r.candidates)
                    .find(|c| c.id == id)
                    .cloned();
                if let Some(mut candidate) = candidate {
                    self.prefs = *prefs;
                    self.snapshot.document.next_id += 1;
                    candidate.id = self.snapshot.document.next_id;
                    candidate.tools.clear();
                    candidate.feedback = None;
                    candidate.ready = false;
                    candidate.error = None;
                    candidate.recoveries = 0;
                    self.snapshot.document.rounds.push(Round {
                        after_message: self.snapshot.document.messages.len(),
                        candidates: vec![candidate.clone()],
                    });
                    self.launch(candidate);
                }
            }
            Command::Stop(id) => {
                if id.is_none() {
                    self.planner = None;
                }
                for (&job, cancel) in &self.jobs {
                    if id.is_none_or(|id| job == id) {
                        cancel.store(true, Ordering::Release);
                        self.snapshot.progress.insert(job, "stopping".into());
                    }
                }
            }
            Command::View(index) => {
                if let Some(round) = self.snapshot.document.rounds.get(index) {
                    for candidate in &round.candidates {
                        if candidate.ready && !self.snapshot.candidates.contains_key(&candidate.id)
                        {
                            match SongCandidate::load_workshop(
                                &self.store.candidate_path(candidate.id),
                                idea(&candidate.direction),
                            ) {
                                Ok(song) => {
                                    self.snapshot
                                        .candidates
                                        .insert(candidate.id, Arc::new(song));
                                }
                                Err(e) => self.snapshot.error = Some(e),
                            }
                        }
                    }
                }
            }
            Command::ReloadPreferences => match auris_session::workshop::load_preferences() {
                Ok(p) => self.snapshot.preferences = p,
                Err(e) => self.snapshot.error = Some(e),
            },
            Command::Memory(enabled) => {
                match auris_session::workshop::update_preferences(|p| p.enabled = enabled) {
                    Ok(p) => self.snapshot.preferences = p,
                    Err(e) => self.snapshot.error = Some(e),
                }
            }
            Command::Forget => {
                self.preference_digest = None;
                self.preference_digest_failed = 0;
                match auris_session::workshop::forget_preferences() {
                    Ok(p) => self.snapshot.preferences = p,
                    Err(e) => self.snapshot.error = Some(e),
                }
            }
            Command::ForgetChoice(conversation, candidate) => {
                match auris_session::workshop::update_preferences(|p| {
                    p.selections
                        .retain(|s| s.conversation != conversation || s.candidate != candidate)
                }) {
                    Ok(p) => self.snapshot.preferences = p,
                    Err(e) => self.snapshot.error = Some(e),
                }
            }
            _ => {}
        }
    }
    /// Apply finished summaries, then start any that have become due while the director is idle.
    fn digest(&mut self) {
        if let Some(job) = &self.conversation_digest
            && let Some(result) = job.poll(&mut self.snapshot.document)
        {
            let to = job.to();
            self.conversation_digest = None;
            match result {
                Ok(()) => self.publish(true),
                Err(e) => {
                    log::warn!("conversation summary discarded: {e}");
                    self.conversation_digest_failed = to;
                }
            }
        }
        if let Some(job) = &self.preference_digest
            && let Some(result) = job.poll()
        {
            let folded = job.folded();
            self.preference_digest = None;
            match result {
                Ok(preferences) => {
                    self.snapshot.preferences = preferences;
                    self.publish(false);
                }
                Err(e) => {
                    log::warn!("preference summary discarded: {e}");
                    self.preference_digest_failed = folded;
                }
            }
        }
        if self.planner.is_some() || !self.prefs.is_configured() {
            return;
        }
        if self.conversation_digest.is_none()
            && let Some(to) = self.snapshot.document.digest_due()
            && to > self.conversation_digest_failed
        {
            self.conversation_digest =
                digest::ConversationDigest::start(&self.prefs, &self.snapshot.document);
            if self.conversation_digest.is_none() {
                self.conversation_digest_failed = to;
            }
        }
        if self.preference_digest.is_none()
            && let Some(folded) = self.snapshot.preferences.digest_due().map(<[_]>::len)
            && folded > self.preference_digest_failed
        {
            self.preference_digest =
                digest::PreferenceDigest::start(&self.prefs, &self.snapshot.preferences);
            if self.preference_digest.is_none() {
                self.preference_digest_failed = folded;
            }
        }
    }
    /// Move off the open conversation `id` to another unarchived one, or to a new one.
    fn leave(&mut self, id: &str) -> Result<(), String> {
        let next = self
            .snapshot
            .threads
            .iter()
            .find(|t| !t.archived && t.id != id)
            .map(|t| t.id.clone());
        self.change_thread(next.as_deref())
    }
    /// Load a conversation that is not open as a parked owner, holding its lease.
    fn park(&mut self, id: &str) -> Result<(), String> {
        let store = self.store.thread(id)?;
        let lease = store.lease()?;
        store.load()?;
        let mut owner = Self::new(store, self.settings.clone(), self.events.clone());
        owner.lease = Some(lease);
        owner.is_parked = true;
        self.parked.insert(id.to_owned(), owner);
        Ok(())
    }
    /// Open conversation `id`, or a new one when absent. A conversation still at work is parked
    /// rather than stopped, and one parked earlier is picked up where it is.
    fn change_thread(&mut self, id: Option<&str>) -> Result<(), String> {
        let mut next = match id.and_then(|id| self.parked.remove(id)) {
            Some(mut owner) => {
                if let Err(error) = owner.store.set_active_thread() {
                    self.parked.insert(owner.thread_id().to_owned(), owner);
                    return Err(error);
                }
                owner.is_parked = false;
                owner.parked_changed = false;
                if let Ok(preferences) = auris_session::workshop::load_preferences() {
                    owner.snapshot.preferences = preferences;
                }
                owner
            }
            None => {
                let store = match id {
                    Some(id) => self.store.thread(id)?,
                    None => self.store.create_thread()?,
                };
                store.load()?;
                let lease = store.lease()?;
                store.load()?;
                let mut next = Self::new(store, self.settings.clone(), self.events.clone());
                next.store.set_active_thread()?;
                next.lease = Some(lease);
                next
            }
        };
        self.store.save(&self.snapshot.document)?;
        next.parked = std::mem::take(&mut self.parked);
        next.handled = self.handled;
        let mut previous = std::mem::replace(self, next);
        if let Ok(threads) = self.store.threads() {
            self.snapshot.threads = threads;
        }
        if previous.busy() {
            previous.is_parked = true;
            self.parked
                .insert(previous.thread_id().to_owned(), previous);
        }
        Ok(())
    }
    fn update_thread(
        &mut self,
        id: &str,
        update: impl FnOnce(&mut Document) -> Result<(), String>,
    ) -> Result<(), String> {
        if id == self.snapshot.document.thread_id {
            update(&mut self.snapshot.document)?;
            self.store.save(&self.snapshot.document)?;
        } else if let Some(owner) = self.parked.get_mut(id) {
            update(&mut owner.snapshot.document)?;
            owner.store.save(&owner.snapshot.document)?;
        } else {
            let store = self.store.thread(id)?;
            let _lease = store.lease()?;
            let mut document = store.load()?;
            update(&mut document)?;
            store.save(&document)?;
        }
        self.snapshot.threads = self.store.threads()?;
        Ok(())
    }
    fn launch(&mut self, mut candidate: Candidate) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.jobs.insert(candidate.id, cancel.clone());
        self.snapshot
            .progress
            .insert(candidate.id, "planning".into());
        let settings = self.settings.clone();
        let prefs = self.prefs.clone();
        let sender = self.job_sender.clone();
        let folder = self.store.candidate_path(candidate.id);
        let parent = candidate
            .parent
            .map(|id| self.store.candidate_path(id).join("song/song.auris"));
        let id = candidate.id;
        let after_message = self
            .snapshot
            .document
            .rounds
            .iter()
            .find(|round| round.candidates.iter().any(|c| c.id == id))
            .map_or(self.snapshot.document.messages.len(), |round| {
                round.after_message
            });
        let references = self
            .snapshot
            .document
            .messages
            .iter()
            .take(after_message)
            .rev()
            .find(|message| !message.attachments.is_empty())
            .map(|message| self.store.owned_attachments(&message.attachments))
            .unwrap_or_default();
        candidate.direction.instructions =
            composition_assignment(&candidate, &self.snapshot.document);
        match std::thread::Builder::new()
            .name(format!("auris-composer-{id}"))
            .spawn(move || {
                let result = recover_candidate(
                    &candidate,
                    &cancel,
                    &sender,
                    wait_for_recovery,
                    |candidate| {
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            compose(
                                candidate,
                                prefs.clone(),
                                settings.clone(),
                                folder.clone(),
                                parent.clone(),
                                &cancel,
                                &sender,
                                &references,
                            )
                        }))
                        .unwrap_or_else(|_| Err(AttemptError::permanent("Candidate worker failed")))
                    },
                )
                .map(Arc::new);
                let _ = sender.send(JobEvent::Done(id, result));
            }) {
            Ok(handle) => self.handles.push(handle),
            Err(e) => {
                self.jobs.remove(&id);
                self.snapshot.progress.remove(&id);
                self.snapshot.error = Some(e.to_string());
            }
        }
    }
}

#[derive(Debug)]
struct AttemptError {
    message: String,
    retryable: bool,
}

impl AttemptError {
    fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

impl From<String> for AttemptError {
    fn from(message: String) -> Self {
        Self {
            message,
            retryable: true,
        }
    }
}

impl From<&str> for AttemptError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

fn recover_candidate<T>(
    candidate: &Candidate,
    cancel: &AtomicBool,
    sender: &mpsc::Sender<JobEvent>,
    mut wait: impl FnMut(u32, &AtomicBool) -> bool,
    mut attempt: impl FnMut(&Candidate) -> Result<T, AttemptError>,
) -> Result<T, String> {
    let mut candidate = candidate.clone();
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Composition stopped".into());
        }
        match attempt(&candidate) {
            Ok(value) => return Ok(value),
            Err(error) => {
                if cancel.load(Ordering::Acquire)
                    || !error.retryable
                    || candidate.recoveries >= AUTO_RECOVERY_LIMIT
                {
                    return Err(error.message);
                }
                candidate.recoveries += 1;
                candidate.error = Some(error.message.clone());
                let _ = sender.send(JobEvent::Recovering(
                    candidate.id,
                    candidate.recoveries,
                    error.message,
                ));
                if !wait(candidate.recoveries, cancel) {
                    return Err("Composition stopped".into());
                }
            }
        }
    }
}

fn wait_for_recovery(count: u32, cancel: &AtomicBool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(1 << count.min(3));
    while Instant::now() < deadline {
        if cancel.load(Ordering::Acquire) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    !cancel.load(Ordering::Acquire)
}

/// A finished candidate's production record, beside its song, for a candidate continuing it.
const RECORD: &str = "production.json";
/// The record as of the latest checkpoint, which a recovery resumes.
const CHECKPOINT_RECORD: &str = "checkpoint/production.json";

/// The production record a candidate starts from, given its round and its parent's record.
///
/// A demo enters the score phase, keeping a parent demo's roles when it revises one. A finishing
/// candidate enters the arrangement on its parent's record, so the demo's melody, harmony and
/// tempo are locked while it is arranged; a parent with no registered roles cannot be finished.
/// An assignment outside a phased round starts with no record, as before.
fn round_record(
    phase: Option<auris_session::workshop::RoundPhase>,
    parent: Option<auris_session::production_record::Record>,
) -> Result<Option<auris_session::production_record::Record>, String> {
    use auris_session::production_record::{Operation, Phase};
    use auris_session::workshop::RoundPhase;
    let (mut record, phase, reason) = match phase {
        None => return Ok(None),
        Some(RoundPhase::Score) => (
            parent.unwrap_or_default(),
            Phase::Score,
            "Workshop demo round: the user compares demos before one is arranged",
        ),
        Some(RoundPhase::Finish) => {
            let record = parent
                .filter(|record| record.roles.is_some())
                .ok_or("Only a demo with registered roles can be finished; select a demo first")?;
            (
                record,
                Phase::Arrangement,
                "Workshop: the user selected this demo to be arranged and mixed",
            )
        }
    };
    record.update(Operation::EnterPhase {
        phase,
        reason: reason.into(),
        writing_order: None,
    })?;
    Ok(Some(record))
}

/// Where a candidate's production record is, given the song file a child opens as its parent.
fn parent_record_path(song: Option<&std::path::Path>) -> Option<PathBuf> {
    Some(song?.parent()?.parent()?.join(RECORD))
}

fn read_record(
    path: &std::path::Path,
) -> Result<auris_session::production_record::Record, AttemptError> {
    let bytes = std::fs::read(path).map_err(|e| AttemptError::permanent(e.to_string()))?;
    serde_json::from_slice(&bytes)
        .map_err(|e| AttemptError::permanent(format!("Production record is unreadable: {e}")))
}

fn keep_record(worker: &crate::Worker, path: &std::path::Path) -> Result<(), AttemptError> {
    let record = worker.production_record()?;
    let bytes = serde_json::to_vec(&record).map_err(|e| AttemptError::permanent(e.to_string()))?;
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder).map_err(|e| AttemptError::permanent(e.to_string()))?;
    }
    auris_session::settings::write_config_bytes(path, &bytes)
        .map_err(|e| AttemptError::permanent(e.to_string()))
}

fn save_checkpoint(session: &mut Session, folder: &std::path::Path) -> Result<(), AttemptError> {
    let path = folder.join("checkpoint/checkpoint.auris");
    if session.path() == Some(path.as_path()) {
        session
            .save(&path)
            .map_err(|e| AttemptError::permanent(format!("Checkpoint save failed: {e}")))?;
    } else {
        std::fs::create_dir_all(folder).map_err(|e| AttemptError::permanent(e.to_string()))?;
        let report = session
            .save_as(&folder.join("checkpoint.auris"))
            .map_err(|e| AttemptError::permanent(format!("Checkpoint save failed: {e}")))?;
        if !report.uncollected.is_empty() {
            return Err(AttemptError::permanent(
                "Checkpoint assets could not be collected",
            ));
        }
    }
    Ok(())
}

fn idea(direction: &Direction) -> SongIdea {
    SongIdea {
        title: direction.title.clone(),
        description: direction.description.clone(),
        preset: "pop-band".into(),
        tempo: 120.0,
        key: "C major".into(),
        density: 0.5,
        lead: "piano".into(),
    }
}

/// The private session one composer works in: the sounds the window has, and nothing more.
///
/// A composer is asked for "the Yamaha Grand Piano out of Fluid R3" in the user's words, and the
/// only place it can look is `search_instruments` on this session. A headless session has no
/// SoundFonts at all, so without the shipped fonts and the user's library here a composer could
/// only answer every such request with a built-in synthesiser and an apologetic track name.
/// Tests leave both out for the reason [`SessionOptions::shipped_fonts`] gives.
/// The earlier candidates a direction may restore from: saved ones, other than the parent,
/// each once, at most [`auris_session::source_material::MAX_SOURCES`]. A model's guess at an
/// ID that names nothing is dropped rather than failing the whole assignment.
fn usable_sources(requested: &[u64], parent: Option<u64>, saved: impl Fn(u64) -> bool) -> Vec<u64> {
    let mut sources = Vec::new();
    for &id in requested {
        if Some(id) != parent && saved(id) && !sources.contains(&id) {
            sources.push(id);
        }
    }
    sources.truncate(auris_session::source_material::MAX_SOURCES);
    sources
}

fn candidate_options(settings: &Settings) -> SessionOptions {
    let installed = !cfg!(test);
    let mut options = SessionOptions::headless()
        .with_balance(false)
        .with_shipped_fonts(installed)
        // An empty dictionary setting means the shipped one, as it does in the window; a
        // headless session would otherwise read kanji lyrics with nothing at all.
        .with_shipped_dictionary(installed)
        .with_plugin_paths(settings.plugin_paths.clone());
    if installed {
        options = options
            .with_soundfont_library(auris_session::config_dir().join("soundfonts.json"))
            .with_performance_presets(
                auris_session::performance_presets::PerformancePresetBook::path(),
            );
    }
    options
}

/// Composes one candidate from a direction, outside any conversation: development tooling for
/// repeating a theme already in the workshop logs after a prompt or tool change.
///
/// The candidate is made exactly as the workshop makes it — the assignment carrying `document`'s
/// conversation, the same isolated session and composer, the same automatic recovery from the
/// checkpoint, the same log in `folder` — so a trial's `composer-log.jsonl` compares line for
/// line with a logged one. Pass the conversation as it stood when the candidate was planned.
/// `parent` continues an earlier song; `progress` hears each stage, tool result and status line
/// as it happens. It is not part of the application: nothing in the window calls it.
pub fn compose_trial(
    direction: Direction,
    document: &Document,
    prefs: AgentPreferences,
    settings: Settings,
    folder: PathBuf,
    parent: Option<PathBuf>,
    mut progress: impl FnMut(String),
) -> Result<SongCandidate, String> {
    let mut candidate = Candidate {
        id: 1,
        parent: None,
        direction,
        ready: false,
        error: None,
        recoveries: 0,
        tools: Default::default(),
        feedback: None,
    };
    candidate.direction.instructions = composition_assignment(&candidate, document);
    let (sender, receiver) = mpsc::channel();
    let cancel = AtomicBool::new(false);
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| {
            recover_candidate(
                &candidate,
                &cancel,
                &sender,
                wait_for_recovery,
                |candidate| {
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        compose(
                            candidate,
                            prefs.clone(),
                            settings.clone(),
                            folder.clone(),
                            parent.clone(),
                            &cancel,
                            &sender,
                            &[],
                        )
                    }))
                    .unwrap_or_else(|_| Err(AttemptError::permanent("Candidate worker failed")))
                },
            )
        });
        while !worker.is_finished() {
            while let Ok(event) = receiver.try_recv() {
                match event {
                    JobEvent::Progress(_, stage) => progress(format!("stage {stage}")),
                    JobEvent::Status(_, status) => progress(format!("status {status}")),
                    JobEvent::Tool(_, name, Some(ok)) => {
                        progress(format!("tool {name} {}", if ok { "ok" } else { "FAILED" }))
                    }
                    JobEvent::Recovering(_, count, error) => {
                        progress(format!("recovering ({count}): {error}"))
                    }
                    _ => {}
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        worker
            .join()
            .map_err(|_| "The composer panicked".to_owned())?
    })
}

// Render and voice inference share a conservative process-wide gate; model turns remain parallel.
static AUDIO_WORK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[expect(
    clippy::too_many_arguments,
    reason = "isolated candidate inputs and worker channels"
)]
fn compose(
    candidate: &Candidate,
    mut prefs: AgentPreferences,
    settings: Settings,
    folder: PathBuf,
    parent: Option<PathBuf>,
    cancel: &AtomicBool,
    sender: &mpsc::Sender<JobEvent>,
    references: &[PathBuf],
) -> Result<SongCandidate, AttemptError> {
    if cancel.load(Ordering::Acquire) {
        return Err("Cancelled".into());
    }
    let mut session = Session::new(candidate_options(&settings)).map_err(|e| e.to_string())?;
    if let Some(dictionary) = &settings.japanese_dictionary {
        session
            .set_japanese_dictionary(Some(dictionary))
            .map_err(|e| e.to_string())?;
    }
    if let Some(parent) = &parent {
        let missing = session.open(parent).map_err(|e| e.to_string())?;
        if !missing.is_empty() {
            return Err(AttemptError::permanent(
                "Parent candidate has missing assets",
            ));
        }
    }
    // The original parent remains the reference for protected material and completion.
    let before = session.project().clone();
    let validate_protected = |session: &Session| -> Result<(), AttemptError> {
        for id in &candidate.direction.protected_tracks {
            let original = before
                .tracks
                .iter()
                .find(|t| t.id.0 == *id)
                .and_then(|t| t.kind.note_clips());
            let current = session
                .project()
                .tracks
                .iter()
                .find(|t| t.id.0 == *id)
                .and_then(|t| t.kind.note_clips());
            if original != current {
                return Err(AttemptError::permanent(format!(
                    "The composer changed protected notes on track {id}"
                )));
            }
        }
        Ok(())
    };
    let checkpoint = folder.join("checkpoint/checkpoint.auris");
    let checkpoint_exists = checkpoint
        .try_exists()
        .map_err(|e| AttemptError::permanent(e.to_string()))?;
    if checkpoint_exists {
        let missing = session
            .open(&checkpoint)
            .map_err(|e| AttemptError::permanent(e.to_string()))?;
        if !missing.is_empty() {
            return Err(AttemptError::permanent(
                "Checkpoint has missing assets; restore them before recovering",
            ));
        }
    }
    prefs.policy = auris_session::agent_policy::Policy {
        mode: auris_session::agent_policy::Mode::Edit,
        allow: vec![],
        // Denying the shell outright starts CLI runtimes without their own shell and file
        // tools, so a composer is never offered one only to have it refused.
        deny: vec![
            "search_internet".into(),
            "edit_project.listen".into(),
            "cli_command".into(),
            "cli_file_change".into(),
            "cli_file_read".into(),
        ],
    };
    prefs.system_prompt = format!(
        "{}\n{}",
        crate::system_prompt(&prefs),
        auris_toolbox::workshop::COMPOSER
    );
    let worker = crate::Worker::spawn(prefs.clone(), None, true, None)?;
    let kept = folder.join(CHECKPOINT_RECORD);
    let record = if checkpoint_exists && kept.is_file() {
        // A recovery resumes the record its checkpoint was saved with, phase and roles included.
        Some(read_record(&kept)?)
    } else {
        // The parent's record sits beside its song: <candidate>/song/song.auris and
        // <candidate>/production.json.
        let parent_record = match parent_record_path(parent.as_deref()) {
            Some(path) if path.is_file() => Some(read_record(&path)?),
            _ => None,
        };
        round_record(candidate.direction.phase, parent_record).map_err(AttemptError::permanent)?
    };
    if let Some(record) = record {
        worker.seed_production_record(record)?;
    }
    let mut previews = preview::Renderer::start(candidate.id, sender.clone())?;
    if candidate.direction.protected_tracks.len() > 128
        || candidate
            .direction
            .protected_tracks
            .iter()
            .any(|id| !before.tracks.iter().any(|t| t.id.0 == *id))
    {
        return Err("Protected track is not present in the parent candidate".into());
    }
    session.begin_agent_request();
    let _ = sender.send(JobEvent::Progress(candidate.id, "planning".into()));
    let assignment = json!({"assignment":candidate.direction.instructions,
    "status":"The user watches this candidate's card and sees only your report_status lines while you work. Call report_status with one short line in the assignment's language each time you start a new step, naming the concrete musical work.",
    "sources":(!candidate.direction.sources.is_empty()).then(|| json!({
        "candidates":candidate.direction.sources,
        "instruction":"These earlier versions are loaded read-only. To bring material back, call source_material list, then restore the named clips exactly; never rewrite remembered notes. Re-sing restored singer clips."
    })),
    "phase":candidate.direction.phase.map(|phase| match phase {
        auris_session::workshop::RoundPhase::Score => auris_toolbox::workshop::DEMO_ROUND,
        auris_session::workshop::RoundPhase::Finish => auris_toolbox::workshop::FINISH_ROUND,
    }),
    "recovery":candidate.error.as_ref().map(|error| json!({
        "last_error":error,
        "instruction":"Inspect the restored score and current resources. Continue only unfinished work; do not duplicate successful edits. Correct the cause of the last failure before completing."
    }))});
    // Earlier versions named by the director, beside this candidate's own folder.
    for &source in &candidate.direction.sources {
        let path = folder
            .parent()
            .unwrap_or(&folder)
            .join(source.to_string())
            .join("song/song.auris");
        session
            .add_source_project(source, &format!("candidate {source}"), &path)
            .map_err(|e| {
                AttemptError::permanent(format!("Source candidate {source} could not be read: {e}"))
            })?;
    }
    let mut log = composer_log::Log::open(&folder);
    log.note(
        "assignment",
        json!({"message":assignment,"backend":prefs.backend,"model":prefs.model}),
    );
    worker.send(
        &json!({"say": assignment.to_string(),"policy":prefs.policy,"attachments":references})
            .to_string(),
    )?;
    let mut deadline = Instant::now() + Duration::from_secs(900);
    let mut denials = 0;
    let mut lint_sends = 0;
    let mut last_failure = String::new();
    let mut tool_calls = BTreeMap::new();
    let completion = loop {
        if cancel.load(Ordering::Acquire) {
            log.note("cancelled", json!(null));
            return Err("Cancelled".into());
        }
        if Instant::now() > deadline {
            log.note("timed_out", json!(null));
            return Err("Candidate production timed out".into());
        }
        // Capturing is cheap; rendering happens separately while the model keeps composing.
        previews.capture(&mut session);
        let event = match worker.try_recv() {
            Ok(event) => event,
            Err(mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            Err(_) => return Err("Candidate worker stopped".into()),
        };
        observe_tool_event(candidate.id, &event, &mut tool_calls, sender);
        log.observe(&event);
        match event["event"].as_str().unwrap_or_default() {
            "permission" => {
                let allowed = auris_session::agent_policy::Operation::parse(
                    event["tool"].as_str().unwrap_or_default(),
                    &event["args"],
                )
                .is_ok_and(|op| {
                    op.name.starts_with("edit_project.")
                        || matches!(
                            op.name.as_str(),
                            "search_instruments" | "instrument_diagnostics" | "production_record"
                        )
                        || auris_toolbox::runtime::live_references()
                            .iter()
                            .any(|t| t.name == op.name)
                });
                // A refusal is an answer the composer can act on: CLI runtimes reach for their
                // own shell and file tools (Claude Code reads oversized tool output back from a
                // file), and one such reach should not discard the whole candidate.
                log.permission(event["tool"].as_str().unwrap_or_default(), allowed);
                worker.send(&json!({"event":"permission_result","id":event["id"],"ok":allowed,
                    "reason":"Isolated candidate: only Auris music tools are available. Do not use shell or file tools; continue with the Auris tools."}).to_string())?;
                if !allowed {
                    denials += 1;
                    if denials > 8 {
                        return Err(AttemptError::permanent(format!(
                            "Candidate operation was denied: {}",
                            event["tool"].as_str().unwrap_or_default()
                        )));
                    }
                }
            }
            "edit" if event["command"]["action"] == "report_status" => {
                // The composer's own words replace the stage guessed from its tool calls.
                let result = auris_session::live_agent::report_status(
                    event["command"]["status"].as_str().unwrap_or_default(),
                );
                if let Ok(status) = &result {
                    let _ = sender.send(JobEvent::Status(candidate.id, status.clone()));
                }
                let reply = match result {
                    Ok(status) => {
                        json!({"event":"edit_result","ok":true,"text":json!({"shown":status}).to_string()})
                    }
                    Err(error) => json!({"event":"edit_result","ok":false,"text":error}),
                };
                worker.send(&reply.to_string())?;
            }
            "edit" => {
                let action = event["command"]["action"].as_str().unwrap_or_default();
                let stage = match action {
                    "edit_harmony" | "suggest_harmony" => "harmony",
                    "edit_notes" | "generate_phrase" => "melody",
                    "sing" | "set_singer_voice" => "sing",
                    "compose" | "add_track" => "compose",
                    _ => "arrange",
                };
                let _ = sender.send(JobEvent::Progress(candidate.id, stage.into()));
                let lock = match event.get("lock") {
                    Some(lock) => Some(
                        serde_json::from_value::<auris_session::phase::Lock>(lock.clone())
                            .map_err(|e| e.to_string())?,
                    ),
                    None => None,
                };
                let result = execute(
                    &mut session,
                    event["command"].clone(),
                    lock.as_ref(),
                    &settings,
                    cancel,
                );
                if result.is_ok() && session.is_dirty() {
                    validate_protected(&session)?;
                    save_checkpoint(&mut session, &folder)?;
                    keep_record(&worker, &folder.join(CHECKPOINT_RECORD))?;
                }
                let reply = match result {
                    Ok(text) if action == "inspect_audio" => {
                        json!({"event":"edit_result","ok":true,"inspection":serde_json::from_str::<serde_json::Value>(&text).map_err(|e|e.to_string())?})
                    }
                    result => {
                        json!({"event":"edit_result","ok":result.is_ok(),"text":result.unwrap_or_else(|e|e)})
                    }
                };
                worker.send(&reply.to_string())?;
            }
            "result" if event["ok"] == false => {
                last_failure = event["text"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(1000)
                    .collect();
            }
            "result" if event["ok"] == true => last_failure.clear(),
            "answer" => {
                // A composer that reports done with warnings left, or with a song short of
                // what its phases require, is sent back to them a bounded number of times:
                // instructions alone are skipped by models that finish fast. What still remains
                // after that is logged, not refused.
                let structure =
                    unmet_structure(&worker.production_record()?, &session, &candidate.direction);
                let lint = session.unresolved_lint(LINT_SEND_BACK_FINDINGS);
                if lint.is_some() || !structure.is_empty() {
                    let unresolved = json!({"lint":lint,"structure":structure});
                    if lint_sends < SEND_BACKS {
                        lint_sends += 1;
                        log.note("send_back", unresolved.clone());
                        let _ = sender.send(JobEvent::Progress(candidate.id, "arrange".into()));
                        deadline += Duration::from_secs(300);
                        worker.send(
                            &json!({"say": send_back(&unresolved).to_string(),"policy":prefs.policy})
                                .to_string(),
                        )?;
                        continue;
                    }
                    log.note("unresolved", unresolved);
                }
                break event["text"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(2000)
                    .collect::<String>();
            }
            "error" => {
                let message = event["message"]
                    .as_str()
                    .unwrap_or("Candidate failed")
                    .to_owned();
                // Repeating a deliberately exhausted allowance automatically would multiply
                // the user's chosen limit. Explicit recovery starts a fresh allowance.
                return Err(if crate::tool_calls::limit_from_error(&message).is_some() {
                    AttemptError::permanent(message)
                } else {
                    message.into()
                });
            }
            "ended" => return Err("Candidate ended without completing".into()),
            _ => {}
        }
    };
    // Join before publishing a finished candidate so no late draft can replace its audition.
    drop(previews);
    session.end_agent_request();
    validate_protected(&session)?;
    let record = worker.production_record()?;
    if candidate.direction.phase == Some(auris_session::workshop::RoundPhase::Score)
        && record.roles.is_none()
    {
        log.note("demo_without_roles", json!(null));
        return Err("The demo did not register its foreground with production_record set_roles; finishing it needs the roles".into());
    }
    keep_record(&worker, &folder.join(RECORD))?;
    if session.project() == &before {
        log.note("unchanged", json!(null));
        return Err(format!(
            "The composer did not make the requested music changes. {completion} {last_failure}"
        )
        .into());
    }
    if !last_failure.is_empty() {
        log.note("unresolved_failure", json!(last_failure));
        return Err(format!("The last tool failure remains unresolved: {last_failure}").into());
    }
    log.note("finished", json!(null));
    drop(log);
    let _ = sender.send(JobEvent::Progress(candidate.id, "render_wait".into()));
    let _gate = audio_gate(cancel)?;
    let _ = sender.send(JobEvent::Progress(candidate.id, "render".into()));
    session
        .finish_workshop_candidate(&folder, idea(&candidate.direction), cancel)
        .map_err(Into::into)
}

/// How many times a composer is sent back to an unfinished song before it is taken as it is.
const SEND_BACKS: usize = 2;

/// How many lint findings a send-back lists; the counts by rule cover the rest.
const LINT_SEND_BACK_FINDINGS: usize = 12;

/// What a song reported finished still lacks: the unmet structural checks of every phase its
/// record reached, and, for anything but a demo, the mix phase itself.
fn unmet_structure(
    record: &auris_session::production_record::Record,
    session: &Session,
    direction: &Direction,
) -> Vec<serde_json::Value> {
    use auris_session::production_record::Phase;
    let mut unmet = auris_session::production_record::unmet_structure(record, session);
    let demo = direction.phase == Some(auris_session::workshop::RoundPhase::Score);
    if !demo
        && record
            .phase
            .as_ref()
            .is_some_and(|state| state.reached < Phase::Mix)
    {
        unmet.push(json!({"check":"mix_reached","unmet":["the song was not arranged and mixed: enter the arrangement and mix phases"]}));
    }
    unmet
}

/// The message that sends a composer back to what its finished song still lacks.
fn send_back(unresolved: &serde_json::Value) -> serde_json::Value {
    json!({
        "host_check": "The host checked the song you reported finished and found it incomplete: lint warnings or errors that remain, structural phase checks that are unmet, or both. Complete it now, then finish with your final report again.",
        "unresolved": unresolved,
        "instruction": "For structure, do the missing work the assignment asks for: write the remaining sections, melody and lyrics, replace the guide, arrange and mix; production_record read lists the phase checks. For lint, run lint_project for the full list with ids; apply_lint_fixes takes the obvious fixes, and a finding without one needs your own edit. A change to locked material needs enter_phase to the phase that owns it, with this check as the reason. A warning you keep on purpose: name it and say why in your final report. Do not redo finished work.",
    })
}

fn audio_gate(cancel: &AtomicBool) -> Result<std::sync::MutexGuard<'static, ()>, String> {
    loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        match AUDIO_WORK.try_lock() {
            Ok(gate) => return Ok(gate),
            Err(std::sync::TryLockError::Poisoned(_)) => return Err("Audio worker failed".into()),
            Err(_) => std::thread::sleep(Duration::from_millis(25)),
        }
    }
}

fn execute(
    session: &mut Session,
    command: serde_json::Value,
    lock: Option<&auris_session::phase::Lock>,
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let action = command["action"].as_str().unwrap_or_default();
    // A file tool aimed only at the candidate's own project workspace stays inside the private
    // candidate, so it is a child capability like any edit; anywhere else on disk is not.
    let workspace_file =
        matches!(
            action,
            "render" | "export_midi" | "import_audio" | "import_midi" | "verify_render"
        ) && serde_json::from_value::<auris_session::live_agent::Command>(command.clone())
            .is_ok_and(|command| command.uses_only_workspace());
    // File exports/imports, playback, nested jobs and network tools are not child capabilities.
    if !workspace_file
        && matches!(
            action,
            "render"
                | "compose"
                | "phrase_variation"
                | "edit_recipe"
                | "listen"
                | "import_transcription"
                | "preview"
                | "export_midi"
                | "import_audio"
                | "import_midi"
                | "audio_assets"
                | "checkpoints"
                | "verify_render"
                | "candidates"
                | "accompaniment"
                | "arrangement"
        )
    {
        return Err("This operation is not available inside candidate production".into());
    }
    if action == "generate_phrase"
        && !matches!(
            command["part"].as_str(),
            Some("chords" | "pad" | "arp" | "drums" | "kick" | "snare" | "hat")
        )
    {
        return Err("Author expressive lead and bass notes directly with edit_notes".into());
    }
    let vocal = matches!(action, "list_voices" | "set_singer_voice" | "sing");
    let command = serde_json::from_value(command).map_err(|e| e.to_string())?;
    if let auris_session::live_agent::Command::InspectAudio {
        start_bar,
        bars,
        track,
    } = command
    {
        let _gate = audio_gate(cancel)?;
        let report = session
            .audio_inspection_job(start_bar, bars, track)?
            .run(cancel)?;
        return serde_json::to_string(&report).map_err(|e| e.to_string());
    }
    if workspace_file {
        // Rendering shares the audio budget with the other candidates' inspections.
        let _gate = audio_gate(cancel)?;
        return session.agent_command_with_plugin_paths(command, &settings.plugin_paths);
    }
    if vocal {
        let _gate = audio_gate(cancel)?;
        let mut job = session.begin_agent_vocal(command, settings.clone())?;
        loop {
            let result = job.run(cancel)?;
            match session.agent_locked(lock, |session| session.continue_agent_vocal(result))? {
                auris_session::live_agent::VocalStep::Work(next) => job = *next,
                auris_session::live_agent::VocalStep::Done(text) => return Ok(text),
            }
        }
    }
    session.agent_command_locked(command, lock, &settings.plugin_paths)
}

#[cfg(test)]
mod tests {
    /// The first snapshot a freshly started workshop publishes.
    fn first_snapshot(workshop: &super::Workshop) -> super::Snapshot {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(snapshot) = workshop.poll() {
                return snapshot;
            }
            assert!(std::time::Instant::now() < deadline, "no snapshot");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn a_demo_starts_in_the_score_and_finishing_it_locks_its_melody() {
        use auris_session::production_record::{Operation, Phase};
        use auris_session::workshop::RoundPhase;
        assert!(super::round_record(None, None).unwrap().is_none());

        let demo = super::round_record(Some(RoundPhase::Score), None)
            .unwrap()
            .unwrap();
        assert_eq!(demo.phase.as_ref().unwrap().phase, Phase::Score);
        assert_eq!(demo.lock(), None);
        // A demo the composer never registered roles for cannot be finished.
        assert!(super::round_record(Some(RoundPhase::Finish), Some(demo.clone())).is_err());
        assert!(super::round_record(Some(RoundPhase::Finish), None).is_err());

        let mut registered = demo;
        registered
            .update(Operation::SetRoles {
                foreground: vec![2],
                guide: vec![4],
                no_foreground: None,
            })
            .unwrap();
        let finish = super::round_record(Some(RoundPhase::Finish), Some(registered.clone()))
            .unwrap()
            .unwrap();
        assert_eq!(
            finish.lock(),
            Some(auris_session::phase::Lock::Arrangement {
                foreground: vec![2]
            })
        );
        assert_eq!(finish.roles.as_ref().unwrap().guide, vec![4]);
        // Revising a demo keeps its roles and stays unlocked.
        let revised = super::round_record(Some(RoundPhase::Score), Some(registered))
            .unwrap()
            .unwrap();
        assert!(revised.roles.is_some() && revised.lock().is_none());
    }

    #[test]
    fn a_reopened_workshop_lists_every_earlier_conversation() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let library = Store::new(directory.path().into());
        let mut earlier = Document::default();
        earlier.rename("Earlier song").unwrap();
        library.save(&earlier).unwrap();
        let second = library.create_thread().unwrap();
        let mut latest = second.load().unwrap();
        latest.rename("Latest song").unwrap();
        second.save(&latest).unwrap();
        second.set_active_thread().unwrap();

        // A later launch opens the same library, whatever document is behind the window.
        let workshop = Workshop::start(library.clone(), Settings::default()).unwrap();
        let snapshot = first_snapshot(&workshop);
        assert_eq!(snapshot.error, None);
        assert_eq!(snapshot.document.title, "Latest song");
        let mut titles: Vec<_> = snapshot.threads.iter().map(|t| t.title.as_str()).collect();
        titles.sort();
        assert_eq!(titles, ["Earlier song", "Latest song"]);
    }

    #[test]
    fn a_second_window_starts_a_new_conversation_instead_of_failing() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let library = Store::new(directory.path().into());
        let mut earlier = Document::default();
        earlier.rename("Earlier song").unwrap();
        library.save(&earlier).unwrap();
        let _first_window = library.active_thread().unwrap().lease().unwrap();

        let workshop = Workshop::start(library.clone(), Settings::default()).unwrap();
        let snapshot = first_snapshot(&workshop);
        assert_eq!(snapshot.error, None);
        assert_ne!(snapshot.document.thread_id, "default");
        assert!(snapshot.threads.iter().any(|t| t.title == "Earlier song"));
        assert_eq!(library.load().unwrap().title, "Earlier song");
    }

    #[test]
    fn loading_another_round_keeps_earlier_auditions_and_errors() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(
            Store::new(directory.path().into()),
            Settings::default(),
            sender,
        );
        for id in [1, 2] {
            owner.snapshot.document.rounds.push(Round {
                after_message: 0,
                candidates: vec![Candidate { id, ..assignment() }],
            });
            owner.snapshot.previews.insert(
                id,
                Preview {
                    revision: 1,
                    audio: Arc::new(auris_session::prelude::AudioBuffer::stereo(100, 100.0)),
                    timeline: Default::default(),
                },
            );
        }
        owner
            .snapshot
            .preview_errors
            .insert(1, "Earlier diagnostic".into());
        owner.command(Command::View(0));
        owner.command(Command::View(1));
        assert_eq!(owner.snapshot.previews.len(), 2);
        assert_eq!(
            owner.snapshot.preview_errors.get(&1).map(String::as_str),
            Some("Earlier diagnostic")
        );
    }

    #[test]
    fn closing_immediately_after_feedback_keeps_the_rating_and_scoped_draft() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (events, _) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), events);
        let mut candidate = assignment();
        candidate.ready = true;
        owner.snapshot.document.rounds.push(Round {
            after_message: 0,
            candidates: vec![candidate],
        });
        let (sender, commands) = mpsc::channel();
        sender
            .send(Command::Feedback(
                "default".into(),
                1,
                auris_session::workshop::Feedback {
                    rating: 5,
                    comment: "Keep this melody".into(),
                },
            ))
            .unwrap();
        sender
            .send(Command::ThreadDraft(
                "default".into(),
                "Next request".into(),
                vec![],
            ))
            .unwrap();
        owner.run(commands, Arc::new(AtomicBool::new(true)));
        let restored = store.load().unwrap();
        assert_eq!(
            restored.rounds[0].candidates[0]
                .feedback
                .as_ref()
                .unwrap()
                .rating,
            5
        );
        assert_eq!(restored.draft, "Next request");
    }
    #[test]
    fn switching_threads_preserves_scoped_drafts_and_archiving_is_reversible() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), sender);
        owner.command(Command::ThreadDraft(
            "default".into(),
            "Original draft".into(),
            vec![],
        ));
        owner.command(Command::NewThread);
        let second = owner.snapshot.document.thread_id.clone();
        assert_ne!(second, "default");
        owner.command(Command::ThreadDraft(
            "default".into(),
            "Stale late draft".into(),
            vec![],
        ));
        assert!(owner.snapshot.document.draft.is_empty());
        owner.command(Command::ThreadDraft(
            second.clone(),
            "Second draft".into(),
            vec![],
        ));
        owner.command(Command::RenameThread(second.clone(), "Second song".into()));
        owner.command(Command::OpenThread("default".into()));
        assert_eq!(owner.snapshot.document.draft, "Original draft");
        owner.command(Command::ArchiveThread(second.clone(), true));
        assert!(
            owner
                .snapshot
                .threads
                .iter()
                .any(|t| t.id == second && t.archived)
        );
        owner.command(Command::ArchiveThread(second.clone(), false));
        owner.command(Command::OpenThread(second.clone()));
        assert_eq!(owner.snapshot.document.draft, "Second draft");
        assert_eq!(owner.snapshot.document.title, "Second song");
        assert_eq!(
            store.active_thread().unwrap().load().unwrap().title,
            "Second song"
        );
    }
    #[test]
    fn a_conversation_at_work_keeps_working_while_another_is_open() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (sender, events) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), sender);
        owner.lease = Some(store.lease().unwrap());
        owner.snapshot.document.rounds.push(Round {
            after_message: 0,
            candidates: vec![assignment()],
        });
        let cancel = Arc::new(AtomicBool::new(false));
        owner.jobs.insert(1, cancel.clone());

        // Leaving a busy conversation is allowed, and its work is neither stopped nor lost.
        owner.command(Command::NewThread);
        let second = owner.snapshot.document.thread_id.clone();
        assert_ne!(second, "default");
        assert_eq!(owner.snapshot.error, None);
        assert!(!cancel.load(Ordering::Acquire));
        assert!(owner.parked.contains_key("default"));
        owner.publish(false);
        let snapshot = events.try_iter().last().unwrap();
        assert_eq!(snapshot.document.thread_id, second);
        assert!(snapshot.working.contains("default"));
        assert!(!snapshot.working.contains(&second));

        // The parked conversation still hears from its composer and saves what it hears.
        let job_sender = owner.parked["default"].job_sender.clone();
        job_sender
            .send(JobEvent::Tool(1, "add_track".into(), None))
            .unwrap();
        owner.tick_parked();
        assert_eq!(
            store.load().unwrap().rounds[0].candidates[0].tools["add_track"].total,
            1
        );

        // A draft written there, and a rename, reach it rather than the open conversation.
        owner.command(Command::ThreadDraft(
            "default".into(),
            "Later idea".into(),
            vec![],
        ));
        owner.command(Command::RenameThread("default".into(), "First song".into()));
        assert!(owner.snapshot.document.draft.is_empty());
        assert!(
            owner
                .snapshot
                .threads
                .iter()
                .any(|t| t.id == "default" && t.title == "First song")
        );

        // Going back picks the same conversation up, workers and all.
        owner.command(Command::OpenThread("default".into()));
        assert_eq!(owner.snapshot.document.thread_id, "default");
        assert_eq!(owner.snapshot.document.draft, "Later idea");
        assert!(owner.jobs.contains_key(&1));
        assert!(owner.parked.is_empty(), "the idle second one is let go");
        job_sender
            .send(JobEvent::Done(1, Err("Composition stopped".into())))
            .unwrap();
        owner.tick();
        assert!(!owner.busy());

        // Once finished in the background, a conversation is released and its lease with it.
        owner.jobs.insert(2, Arc::new(AtomicBool::new(false)));
        owner.command(Command::OpenThread(second.clone()));
        let job_sender = owner.parked["default"].job_sender.clone();
        job_sender
            .send(JobEvent::Done(2, Err("Composition stopped".into())))
            .unwrap();
        owner.tick_parked();
        assert!(owner.parked.is_empty());
        drop(
            store
                .lease()
                .expect("the finished conversation's lease is free"),
        );
    }
    #[test]
    fn deleting_a_conversation_at_work_stops_it_first() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), sender);
        owner.command(Command::NewThread);
        let second = owner.snapshot.document.thread_id.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        owner.jobs.insert(1, cancel.clone());
        owner.command(Command::OpenThread("default".into()));
        assert!(owner.parked.contains_key(&second));
        owner.command(Command::DeleteThread(second.clone()));
        assert_eq!(owner.snapshot.error, None);
        assert!(cancel.load(Ordering::Acquire));
        assert!(owner.parked.is_empty());
        assert!(owner.snapshot.threads.iter().all(|t| t.id != second));
    }
    #[test]
    fn a_message_sent_to_a_conversation_not_open_is_delivered_there() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), sender);
        owner.command(Command::NewThread);
        let second = owner.snapshot.document.thread_id.clone();
        // An unconfigured model is refused, so nothing runs; the message still lands in the
        // conversation it was written for, and the open one is untouched.
        owner.command(Command::SendTo(
            "default".into(),
            "   ".into(),
            Box::default(),
            vec![],
        ));
        assert_eq!(owner.snapshot.document.thread_id, second);
        assert_eq!(owner.snapshot.error, None);
        assert!(owner.parked.contains_key("default"));
        owner.tick_parked();
        assert!(
            owner.parked.is_empty(),
            "an idle conversation is not kept loaded"
        );
        drop(store.lease().expect("and its lease is released"));
    }
    #[test]
    fn deleting_the_open_conversation_moves_to_another_and_removes_it_from_disk() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), sender);
        owner.command(Command::ThreadDraft(
            "default".into(),
            "Keep me".into(),
            vec![],
        ));
        owner.command(Command::NewThread);
        let second = owner.snapshot.document.thread_id.clone();
        owner.command(Command::DeleteThread(second.clone()));
        assert_eq!(owner.snapshot.error, None);
        assert_eq!(owner.snapshot.document.thread_id, "default");
        assert_eq!(owner.snapshot.document.draft, "Keep me");
        assert!(owner.snapshot.threads.iter().all(|t| t.id != second));
        assert!(
            !directory
                .path()
                .join("workshop/threads")
                .join(&second)
                .exists()
        );

        // Deleting the last one leaves a fresh, empty conversation open.
        owner.command(Command::DeleteThread("default".into()));
        assert_eq!(owner.snapshot.error, None);
        assert_ne!(owner.snapshot.document.thread_id, "default");
        assert!(owner.snapshot.document.draft.is_empty());
        assert_eq!(owner.snapshot.threads.len(), 1);
        assert_eq!(
            store.active_thread().unwrap().identity(),
            owner.store.identity()
        );
    }
    #[test]
    fn feedback_is_persisted_and_attributed_in_future_composer_context() {
        use super::*;
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().into());
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), sender);
        let mut candidate = assignment();
        candidate.ready = true;
        owner.snapshot.document.rounds.push(Round {
            after_message: 0,
            candidates: vec![candidate],
        });
        owner.command(Command::Feedback(
            "default".into(),
            1,
            auris_session::workshop::Feedback {
                rating: 4,
                comment: "Keep the melody, soften the drums".into(),
            },
        ));
        owner.publish(true);
        let restored = store.load().unwrap();
        assert_eq!(
            restored.rounds[0].candidates[0]
                .feedback
                .as_ref()
                .unwrap()
                .rating,
            4
        );
        let context: serde_json::Value =
            serde_json::from_str(&composition_assignment(&assignment(), &restored)).unwrap();
        assert_eq!(context["user_feedback"][0]["candidate"], 1);
        assert_eq!(
            context["user_feedback"][0]["comment"],
            "Keep the melody, soften the drums"
        );
        assert!(owner.planner.is_none());
        assert!(owner.jobs.is_empty());
    }
    #[test]
    fn tool_observations_count_parallel_calls_once_and_accept_result_only_events() {
        use super::*;
        let (sender, receiver) = mpsc::channel();
        let mut calls = BTreeMap::new();
        for event in [
            json!({"event":"call","call_id":"a","tool":"edit_notes"}),
            json!({"event":"call","call_id":"a","tool":"edit_notes"}),
            json!({"event":"call","call_id":"b","tool":"edit_notes"}),
            json!({"event":"result","call_id":"a","tool":"edit_notes","ok":true}),
            json!({"event":"result","call_id":"a","tool":"edit_notes","ok":true}),
            json!({"event":"result","call_id":"b","tool":"edit_notes","ok":false}),
            json!({"event":"result","call_id":"c","tool":"inspect_project","ok":true}),
        ] {
            observe_tool_event(1, &event, &mut calls, &sender);
        }
        let observed: Vec<_> = receiver
            .try_iter()
            .map(|event| match event {
                JobEvent::Tool(id, name, result) => (id, name, result),
                _ => panic!("Expected a tool observation"),
            })
            .collect();
        assert_eq!(
            observed,
            vec![
                (1, "edit_notes".into(), None),
                (1, "edit_notes".into(), None),
                (1, "edit_notes".into(), Some(true)),
                (1, "edit_notes".into(), Some(false)),
                (1, "inspect_project".into(), None),
                (1, "inspect_project".into(), Some(true)),
            ]
        );
    }
    use super::*;
    #[test]
    #[ignore = "real model inference; requires AURIS_WORKSHOP_EVAL_OUTPUT and AURIS_WORKSHOP_EVAL_MODEL"]
    fn installed_composer_finishes_music_and_publishes_audio_before_completion() {
        let folder = PathBuf::from(
            std::env::var_os("AURIS_WORKSHOP_EVAL_OUTPUT").expect("fresh output path"),
        );
        assert!(folder.is_absolute() && !folder.exists());
        std::fs::create_dir_all(&folder).unwrap();
        let prefs = AgentPreferences {
            backend: std::env::var("AURIS_WORKSHOP_EVAL_BACKEND")
                .unwrap_or_else(|_| "codex".into()),
            model: std::env::var("AURIS_WORKSHOP_EVAL_MODEL").expect("explicit model"),
            ..Default::default()
        };
        let mut candidate = assignment();
        candidate.direction.instructions = "朝の散歩を描く8小節のオリジナル器楽曲を作って。120 BPM、4/4、ハ長調。前半4小節と後半4小節に主題のつながりと対比を作り、ピアノの主旋律、ベース、控えめなドラムで、最後は余韻を残して終わって。内蔵音源のみ。歌、外部評価、ファイル書き出しは不要。曲全体の譜面とミックスを確認して完成させて。".into();
        let (sender, events) = mpsc::channel();
        let started = Instant::now();
        let collector = std::thread::spawn(move || {
            events.into_iter().map(|event| match event {
                JobEvent::Preview(id, Ok(preview)) => json!({"candidate":id,"event":"preview","elapsed_seconds":started.elapsed().as_secs_f64(),"revision":preview.revision,"seconds":preview.audio.duration_seconds(),"peak":preview.audio.peak()}),
                JobEvent::Preview(id, Err(error)) => json!({"candidate":id,"event":"preview_error","error":error}),
                JobEvent::Progress(id, stage) => json!({"candidate":id,"event":"progress","stage":stage,"elapsed_seconds":started.elapsed().as_secs_f64()}),
                _ => json!({"event":"other"}),
            }).collect::<Vec<_>>()
        });
        let result = compose(
            &candidate,
            prefs.clone(),
            Settings::default(),
            folder.join("candidate"),
            None,
            &AtomicBool::new(false),
            &sender,
            &[],
        );
        drop(sender);
        let events = collector.join().unwrap();
        let score_seconds = result.as_ref().ok().map(|_| {
            let mut check = Session::new(SessionOptions::headless()).unwrap();
            check
                .open(&folder.join("candidate/song/song.auris"))
                .unwrap();
            check.project().duration_seconds()
        });
        let report = json!({"backend":prefs.backend,"model":prefs.model,"assignment":candidate.direction.instructions,"elapsed_seconds":started.elapsed().as_secs_f64(),"error":result.as_ref().err().map(|e| &e.message),"events":events,"outline":result.as_ref().ok().map(SongCandidate::workshop_outline),"score_seconds":score_seconds,"seconds":result.as_ref().ok().map(|c| c.audio.duration_seconds()),"peak":result.as_ref().ok().map(|c| c.audio.peak()),"listening":"unassessed"});
        std::fs::write(
            folder.join("report.json"),
            serde_json::to_vec_pretty(&report).unwrap(),
        )
        .unwrap();
        std::fs::write(
            folder.join("system-prompt.txt"),
            format!(
                "{}\n{}",
                crate::default_system_prompt(),
                auris_toolbox::workshop::COMPOSER
            ),
        )
        .unwrap();
        assert!(result.is_ok(), "{}", report);
        assert!(
            events.iter().any(|event| event["event"] == "preview"),
            "No intermediate audio: {report}"
        );
        assert!(
            report["score_seconds"]
                .as_f64()
                .is_some_and(|seconds| (seconds - 16.0).abs() < 0.01),
            "Unexpected duration: {report}"
        );
    }

    #[test]
    fn director_does_not_leak_its_read_only_role_into_composer_assignments() {
        let answer =
            json!({"role":"assistant","content":"{\"reply\":\"Which mood?\",\"directions\":[]}"})
                .to_string();
        let (url, seen) =
            crate::tests::mock_server(vec![crate::tests::completion(&answer, "stop")]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let mut director =
            crate::song_ideas::SongPlanner::conversation(prefs, "Make three short songs".into())
                .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = director.poll_text() {
                assert!(decision(&result.unwrap()).unwrap().directions.is_empty());
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = seen.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(!requests[0].contains("Current Auris permission mode"));
    }

    #[test]
    fn unselected_single_song_is_inherited_by_followup_composer_after_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().into());
        let cancel = AtomicBool::new(false);
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        execute(
            &mut session,
            json!({"action":"add_track","name":"Original drums","kind":"drum"}),
            None,
            &Settings::default(),
            &cancel,
        )
        .unwrap();
        execute(
            &mut session,
            json!({"action":"generate_phrase","track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}),
            None,
            &Settings::default(),
            &cancel,
        ).unwrap();
        session
            .finish_workshop_candidate(
                &store.candidate_path(1),
                idea(&assignment().direction),
                &cancel,
            )
            .unwrap();
        let original_tracks = session.project().tracks.clone();
        let parent_path = store.candidate_path(1).join("song/song.auris");
        let original_file = std::fs::read(&parent_path).unwrap();
        drop(session);
        let mut parent = assignment();
        parent.ready = true;
        let mut failed = assignment();
        failed.id = 2;
        failed.error = Some("Previous revision started without the original score".into());
        store
            .save(&Document {
                next_id: 2,
                rounds: vec![
                    Round {
                        after_message: 0,
                        candidates: vec![parent],
                    },
                    Round {
                        after_message: 0,
                        candidates: vec![failed],
                    },
                ],
                ..Default::default()
            })
            .unwrap();
        let director = json!({"role":"assistant","content":json!({
            "reply":"Keep the arrangement and slow it down.", "directions":[assignment().direction]
        }).to_string()})
        .to_string();
        let edit = json!({"role":"assistant","tool_calls":[{
            "id":"tempo", "type":"function", "function":{
                "name":"edit_timeline", "arguments":json!({"operation":{"kind":"set_tempo","at_tick":0,"bpm":90.0}}).to_string()
            }
        }]}).to_string();
        let answer = json!({"role":"assistant","content":"Finished the tempo change."}).to_string();
        let (url, seen) = crate::tests::mock_server(vec![
            crate::tests::completion(&director, "stop"),
            crate::tests::completion(&edit, "tool_calls"),
            crate::tests::completion(&answer, "stop"),
        ]);
        let (events, snapshots) = mpsc::channel();
        let mut owner = Owner::new(store.clone(), Settings::default(), events);
        owner.command(Command::Send(
            "Keep the arrangement, but slow it to 90 BPM.".into(),
            Box::new(AgentPreferences {
                provider: "openai".into(),
                model: "mock".into(),
                url,
                ..Default::default()
            }),
            vec![],
        ));
        assert_eq!(
            owner.planned_parent,
            Some(1),
            "a followup must start from the saved song"
        );
        let (commands, receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || owner.run(receiver, stop));
        let deadline = Instant::now() + Duration::from_secs(30);
        let snapshot = loop {
            let snapshot = snapshots
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
            if snapshot.document.rounds.len() == 3 && snapshot.progress.is_empty() {
                break snapshot;
            }
        };
        stopped.store(true, Ordering::Release);
        drop(commands);
        thread.join().unwrap();
        let child = &snapshot.document.rounds[2].candidates[0];
        assert!(child.ready, "{:?}", child.error);
        assert_eq!(child.parent, Some(1));
        assert_eq!(snapshot.document.selected, None);
        let mut restored = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        restored
            .open(&store.candidate_path(3).join("song/song.auris"))
            .unwrap();
        assert_eq!(restored.project().tracks, original_tracks);
        assert_eq!(restored.project().bpm(), 90.0);
        assert_eq!(std::fs::read(parent_path).unwrap(), original_file);
        let requests = seen.lock().unwrap();
        let request: serde_json::Value = serde_json::from_str(&requests[0]).unwrap();
        let prompt: serde_json::Value = serde_json::from_str(
            request["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(prompt["selected"]["id"], 1);
        assert_eq!(
            prompt["selected_score"]["tracks"][0]["name"],
            "Original drums"
        );
    }

    #[test]
    fn additional_request_includes_previous_candidates_without_requiring_selection() {
        let answer = json!({"role":"assistant","content":json!({
            "reply":"Which part would you like to change?", "directions":[]
        }).to_string()})
        .to_string();
        let (url, seen) =
            crate::tests::mock_server(vec![crate::tests::completion(&answer, "stop")]);
        let dir = tempfile::tempdir().unwrap();
        let reference = dir.path().join("lyrics.txt");
        std::fs::write(&reference, "LYRIC_REFERENCE_夜の雨").unwrap();
        let (events, _) = mpsc::channel();
        let mut owner = Owner::new(Store::new(dir.path().into()), Settings::default(), events);
        let mut old = assignment();
        old.direction.title = "Earlier piano idea".into();
        old.direction.description = "Sparse piano over a steady pulse".into();
        owner.snapshot.document.rounds.push(Round {
            after_message: 0,
            candidates: vec![old],
        });
        owner.command(Command::Send(
            "None of these fit; offer another approach".into(),
            Box::new(AgentPreferences {
                provider: "openai".into(),
                model: "mock".into(),
                url,
                ..Default::default()
            }),
            vec![reference.clone()],
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        std::fs::remove_file(&reference).unwrap();
        let planner = owner
            .planner
            .as_mut()
            .expect("director started without a selection");
        loop {
            if let Some(result) = planner.poll_text() {
                assert!(result.is_ok(), "{result:?}");
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(owner.snapshot.document.selected, None);
        assert_eq!(owner.snapshot.document.rounds.len(), 1);
        let requests = seen.lock().unwrap();
        assert!(requests[0].contains("candidate_history"));
        // The director is told the label the user sees, so "1A" means the same to both.
        assert!(
            requests[0].contains(r#""label":"1A""#) || requests[0].contains(r#"\"label\":\"1A\""#)
        );
        assert!(requests[0].contains("Earlier piano idea"));
        assert!(requests[0].contains("Sparse piano over a steady pulse"));
        assert!(requests[0].contains("LYRIC_REFERENCE_夜の雨"));
        let paths = &owner.snapshot.document.messages[0].attachments;
        assert_eq!(paths.len(), 1);
        assert_eq!(
            std::fs::read_to_string(&paths[0]).unwrap(),
            "LYRIC_REFERENCE_夜の雨"
        );
    }

    #[test]
    fn editor_handoff_keeps_readable_turns_and_provider_memory() {
        let dir = tempfile::tempdir().unwrap();
        let document = Document {
            messages: vec![
                Message {
                    choices: None,
                    references: vec![],
                    attachments: Vec::new(),
                    user: true,
                    text: "Keep the melody".into(),
                },
                Message {
                    choices: None,
                    references: vec![],
                    attachments: Vec::new(),
                    user: false,
                    text: "I will change only the accompaniment".into(),
                },
            ],
            draft: "Make the chorus brighter".into(),
            ..Default::default()
        };
        let (store, id) = handoff(
            dir.path().into(),
            false,
            Store::new(dir.path().join("library")),
            document,
        )
        .unwrap();
        let conversation = store.load(&id).unwrap();
        assert_eq!(conversation.transcript.messages.len(), 2);
        assert_eq!(conversation.transcript.draft, "Make the chorus brighter");
        let memory = crate::memory::Memory::load(&store.memory_path(&id).unwrap()).unwrap();
        assert_eq!(memory.turns[0].user, "Keep the melody");
        assert_eq!(
            memory.turns[0].answer,
            "I will change only the accompaniment"
        );
    }
    #[test]
    fn music_references_reach_the_director_composer_and_saved_history() {
        use auris_session::midi_reference::MidiReference;
        use auris_session::prelude::{Note, SignatureMap, TempoMap, Ticks};
        let dir = tempfile::tempdir().unwrap();
        let mut score = MidiReference::new(
            "Melody".into(),
            TempoMap::constant(120.),
            SignatureMap::default(),
        );
        let mut note = Note::new(61, Ticks(240), Ticks(720));
        note.lyric = "きみ".into();
        score.tracks[0].notes.push(note);
        let midi = score.save_snapshot(dir.path()).unwrap();
        let chords = auris_session::music_reference::save_snapshot(
            &json!({"reference_type":"chord_progression","chords":["C/E","G7"]}),
            dir.path(),
            "chords",
        )
        .unwrap();
        let sound = auris_session::music_reference::save_snapshot(&json!({"reference_type":"instrument_choice","instrument_id":"test.piano","preset":{"bank":3,"patch":8}}), dir.path(), "sound").unwrap();
        let paths = vec![midi.clone(), chords, sound];
        let answer = json!({"role":"assistant","content":"{\"reply\":\"I have the references\",\"directions\":[]}"}).to_string();
        let (url, seen) =
            crate::tests::mock_server(vec![crate::tests::completion(&answer, "stop")]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let (events, _) = mpsc::channel();
        let editor_folder = dir.path().join("editor");
        // The library lives apart from the editor's project, as `Store::library` does.
        let store = Store::new(dir.path().join("library"));
        let mut owner = Owner::new(store.clone(), Settings::default(), events);
        owner.command(Command::Send(
            "Use these references".into(),
            Box::new(prefs),
            paths.clone(),
        ));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(result) = owner.planner.as_mut().unwrap().poll_text() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(10));
        }
        let requests = seen.lock().unwrap();
        for expected in ["きみ", "C/E", "test.piano"] {
            assert!(requests[0].contains(expected), "{expected}");
        }
        drop(requests);
        let assignment = composition_assignment(&assignment(), &owner.snapshot.document);
        for expected in ["きみ", "C/E", "test.piano"] {
            assert!(assignment.contains(expected));
        }
        std::fs::write(&midi, b"changed after submission").unwrap();
        store.save(&owner.snapshot.document).unwrap();
        let restored = store.load().unwrap();
        assert!(
            restored.messages[0].references[0]
                .text
                .contains("\"pitch\":61")
        );
        let (editor, id) = handoff(editor_folder, false, store.clone(), restored).unwrap();
        let conversation = editor.load(&id).unwrap();
        let auris_session::conversations::ConversationContent::User { attachments, .. } =
            &conversation.transcript.messages[0].content
        else {
            panic!("Expected a user turn")
        };
        assert_eq!(attachments.len(), paths.len());
        let archived_score = MidiReference::read(&attachments[0]).unwrap();
        assert_eq!(archived_score.tracks[0].notes[0].pitch, 61);
        assert_eq!(archived_score.tracks[0].notes[0].lyric, "きみ");
    }

    #[test]
    fn invalid_midi_keeps_the_workshop_draft_and_does_not_submit_a_partial_turn() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.mid");
        std::fs::write(&path, b"invalid MIDI").unwrap();
        let (events, _) = mpsc::channel();
        let mut owner = Owner::new(Store::new(dir.path().into()), Settings::default(), events);
        owner.command(Command::Send(
            "Use this melody".into(),
            Box::default(),
            vec![path.clone()],
        ));
        assert!(owner.snapshot.document.messages.is_empty());
        assert!(owner.snapshot.error.is_some());
        assert_eq!(owner.snapshot.document.draft, "Use this melody");
        assert_eq!(owner.snapshot.document.draft_attachments, vec![path]);
        assert!(owner.planner.is_none());
    }

    #[test]
    fn sources_keep_saved_earlier_candidates_once_and_never_the_parent() {
        let sources = usable_sources(&[3, 4, 3, 9, 1, 2, 5, 6], Some(4), |id| id != 9);
        assert_eq!(sources, [3, 1, 2, 5]);
    }

    #[test]
    fn a_composer_restores_from_a_named_source_candidate() {
        let root = tempfile::tempdir().unwrap();
        let mut earlier = Session::new(SessionOptions::headless()).unwrap();
        let track = earlier.add_singer_track("Voice");
        let clip = earlier
            .add_midi_clip(
                track,
                "Chorus",
                auris_session::prelude::Ticks::ZERO,
                auris_session::prelude::Ticks::QUARTER,
            )
            .unwrap();
        earlier
            .add_note(
                clip,
                auris_session::prelude::Note::new(
                    72,
                    auris_session::prelude::Ticks::ZERO,
                    auris_session::prelude::Ticks::QUARTER,
                ),
            )
            .unwrap();
        let saved = earlier
            .save_as(&root.path().join("3/song/song.auris"))
            .unwrap();
        assert!(saved.document.ends_with("3/song/song.auris"));
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        session
            .add_source_project(3, "candidate 3", &saved.document)
            .unwrap();
        let listed = session
            .agent_source_material(auris_session::source_material::Operation::List)
            .unwrap();
        assert!(listed.contains("\"in_current\":false"), "{listed}");
    }

    fn assignment() -> Candidate {
        Candidate {
            id: 1,
            parent: None,
            ready: false,
            error: None,
            recoveries: 0,
            tools: Default::default(),
            feedback: None,
            direction: Direction {
                title: "Test".into(),
                description: "A short instrumental".into(),
                instructions: "Create a short chiptune instrumental".into(),
                protected_tracks: vec![],
                sources: vec![],
                phase: None,
            },
        }
    }
    #[test]
    fn selection_records_preference_and_replies_without_composing() {
        const CHILD: &str = "AURIS_SELECTION_TEST_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let config = tempfile::tempdir().unwrap();
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "workshop::tests::selection_records_preference_and_replies_without_composing",
                    "--nocapture",
                ])
                .env(CHILD, "1")
                .env(auris_session::CONFIG_DIR_VAR, config.path())
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().into());
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let cancel = AtomicBool::new(false);
        execute(
            &mut session,
            json!({"action":"add_track","name":"Kit","kind":"drum"}),
            None,
            &Settings::default(),
            &cancel,
        )
        .unwrap();
        execute(&mut session, json!({"action":"generate_phrase","track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}), None, &Settings::default(), &cancel).unwrap();
        let song = session
            .finish_workshop_candidate(
                &store.candidate_path(1),
                idea(&assignment().direction),
                &cancel,
            )
            .unwrap();
        // Even an over-eager director must not launch another batch on selection.
        let directions: Vec<_> = (0..3)
            .map(|i| Direction {
                title: format!("Option {i}"),
                ..assignment().direction
            })
            .collect();
        let reply = "Selected.\n1. **Add bass**: a walking line\n2. Vary the drums\n3. Build a chorus\nWhich next?";
        let answer = json!({"role":"assistant","content":json!({"reply":reply,"directions":directions}).to_string()}).to_string();
        let (url, requests) =
            crate::tests::mock_server(vec![crate::tests::completion(&answer, "stop")]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let (events, snapshots) = mpsc::channel();
        let mut owner = Owner::new(store, Settings::default(), events);
        let mut selected = assignment();
        selected.ready = true;
        owner.snapshot.document.rounds.push(Round {
            after_message: 0,
            candidates: vec![selected.clone(), Candidate { id: 2, ..selected }],
        });
        owner.snapshot.candidates.insert(1, Arc::new(song));
        owner.command(Command::Select(
            1,
            Box::new(prefs.clone()),
            "Continue with Test".into(),
        ));
        owner.command(Command::Select(
            1,
            Box::new(prefs),
            "Duplicate click".into(),
        ));
        assert_eq!(owner.snapshot.document.messages.len(), 1);
        let saved = auris_session::workshop::load_preferences().unwrap();
        assert_eq!(saved.selections.len(), 1);
        assert_eq!(saved.selections[0].candidate, 1);
        let (commands, receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        let thread = std::thread::spawn(move || owner.run(receiver, stop));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let snapshot = snapshots
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap();
            if snapshot.document.messages.last().is_some_and(|m| !m.user) {
                let message = snapshot.document.messages.last().unwrap();
                // The numbered list became the card; the reply keeps only its prose.
                assert_eq!(message.text, "Selected.\nWhich next?");
                let card = message.choices.as_ref().expect("suggestions as a card");
                assert!(card.multiple);
                assert_eq!(card.question, "Which next?");
                assert_eq!(card.options[0].label, "Add bass");
                assert_eq!(card.options[0].detail.as_deref(), Some("a walking line"));
                assert_eq!(card.options.len(), 3);
                assert_eq!(snapshot.document.selected, Some(1));
                assert_eq!(snapshot.document.rounds.len(), 1);
                assert!(snapshot.progress.is_empty());
                break;
            }
            assert!(snapshot.error.is_none(), "{:?}", snapshot.error);
        }
        stopped.store(true, Ordering::Release);
        drop(commands);
        thread.join().unwrap();
        let request = &requests.lock().unwrap()[0];
        assert!(request.contains("candidate_selected"));
        assert!(request.contains("preference_recorded"));
    }

    #[test]
    fn automatic_recovery_is_bounded_and_preserves_failure_context() {
        let (sender, receiver) = mpsc::channel();
        let mut attempts = 0;
        let result = recover_candidate::<()>(
            &assignment(),
            &AtomicBool::new(false),
            &sender,
            |_, _| true,
            |candidate| {
                if attempts > 0 {
                    assert_eq!(candidate.error.as_deref(), Some("temporary failure"));
                }
                attempts += 1;
                Err("temporary failure".into())
            },
        );
        assert_eq!(result.unwrap_err(), "temporary failure");
        assert_eq!(attempts, AUTO_RECOVERY_LIMIT + 1);
        let counts: Vec<_> = receiver
            .try_iter()
            .filter_map(|event| match event {
                JobEvent::Recovering(_, count, _) => Some(count),
                _ => None,
            })
            .collect();
        assert_eq!(counts, vec![1, 2, 3]);
    }

    #[test]
    fn recovery_stops_after_success_denial_or_cancellation() {
        let (sender, _) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let mut attempts = 0;
        let value = recover_candidate(
            &assignment(),
            &cancel,
            &sender,
            |_, _| true,
            |_| {
                attempts += 1;
                if attempts == 1 {
                    Err("connection lost".into())
                } else {
                    Ok(42)
                }
            },
        )
        .unwrap();
        assert_eq!((value, attempts), (42, 2));
        attempts = 0;
        assert!(
            recover_candidate::<()>(
                &assignment(),
                &cancel,
                &sender,
                |_, _| panic!("must not wait"),
                |_| {
                    attempts += 1;
                    Err(AttemptError::permanent("denied"))
                }
            )
            .is_err()
        );
        assert_eq!(attempts, 1);
        attempts = 0;
        assert!(
            recover_candidate::<()>(
                &assignment(),
                &cancel,
                &sender,
                |_, cancel| {
                    cancel.store(true, Ordering::Release);
                    false
                },
                |_| {
                    attempts += 1;
                    Err("temporary failure".into())
                }
            )
            .is_err()
        );
        assert_eq!(attempts, 1);
    }

    #[test]
    fn explicit_recovery_resets_persisted_budget_without_replacing_candidate() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().into());
        let mut candidate = assignment();
        candidate.recoveries = AUTO_RECOVERY_LIMIT;
        candidate.error = Some("previous failure".into());
        store
            .save(&Document {
                rounds: vec![Round {
                    after_message: 0,
                    candidates: vec![candidate],
                }],
                ..Default::default()
            })
            .unwrap();
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(store, Settings::default(), sender);
        let prefs = AgentPreferences {
            provider: "invalid-test-provider".into(),
            ..Default::default()
        };
        owner.command(Command::Recover(1, Box::new(prefs)));
        assert_eq!(owner.snapshot.document.rounds.len(), 1);
        let candidate = &owner.snapshot.document.rounds[0].candidates[0];
        assert_eq!((candidate.id, candidate.recoveries), (1, 0));
        assert_eq!(candidate.error.as_deref(), Some("previous failure"));
        assert_eq!(owner.prefs.provider, "invalid-test-provider");
        owner.jobs[&1].store(true, Ordering::Release);
        for handle in owner.handles.drain(..) {
            handle.join().unwrap();
        }
    }

    #[test]
    fn silent_candidate_recovers_with_real_tools_in_the_same_track() {
        let call = |id: &str, name: &str, args: serde_json::Value| {
            json!({"role":"assistant","tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}}]}).to_string()
        };
        let setup = call("setup", "add_track", json!({"name":"Kit","kind":"drum"}));
        let phrase = call(
            "phrase",
            "generate_phrase",
            json!({"track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}),
        );
        let answer = json!({"role":"assistant","content":"Finished"}).to_string();
        let (url, seen) = crate::tests::mock_server(vec![
            crate::tests::completion(&setup, "tool_calls"),
            crate::tests::completion(&answer, "stop"),
            crate::tests::completion(&phrase, "tool_calls"),
            crate::tests::completion(&answer, "stop"),
        ]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let (sender, receiver) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let result = recover_candidate(
            &assignment(),
            &cancel,
            &sender,
            |_, _| true,
            |candidate| {
                compose(
                    candidate,
                    prefs.clone(),
                    Settings::default(),
                    dir.path().join("1"),
                    None,
                    &cancel,
                    &sender,
                    &[],
                )
                .inspect_err(|error| eprintln!("Recovery attempt failed: {error:?}"))
            },
        )
        .unwrap();
        assert!(result.audio.peak() > 1e-8);
        assert_eq!(
            result.workshop_outline()["tracks"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            receiver
                .try_iter()
                .any(|event| matches!(event, JobEvent::Recovering(1, 1, _)))
        );
        assert_eq!(seen.lock().unwrap().len(), 4);
    }

    #[test]
    fn checkpoint_survives_worker_failure_and_resumes_without_duplicate_tracks() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("1");
        let mut session = Session::new(SessionOptions::headless().with_balance(false)).unwrap();
        let cancel = AtomicBool::new(false);
        execute(
            &mut session,
            json!({"action":"add_track","name":"Kit","kind":"drum"}),
            None,
            &Settings::default(),
            &cancel,
        )
        .unwrap();
        execute(&mut session, json!({"action":"generate_phrase","track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}), None, &Settings::default(), &cancel).unwrap();
        save_checkpoint(&mut session, &folder).unwrap();
        let original = session.project().tracks.clone();
        drop(session);
        let answer = json!({"role":"assistant","content":"Verified the existing score; complete."})
            .to_string();
        let (url, seen) =
            crate::tests::mock_server(vec![crate::tests::completion(&answer, "stop")]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let mut candidate = assignment();
        candidate.error = Some("Connection interrupted after writing notes".into());
        let (sender, _) = mpsc::channel();
        let result = compose(
            &candidate,
            prefs,
            Settings::default(),
            folder.clone(),
            None,
            &cancel,
            &sender,
            &[],
        )
        .unwrap();
        assert!(result.audio.peak() > 1e-8);
        let mut restored = Session::new(SessionOptions::headless()).unwrap();
        restored.open(&folder.join("song/song.auris")).unwrap();
        assert_eq!(restored.project().tracks, original);
        assert!(seen.lock().unwrap()[0].contains("Connection interrupted after writing notes"));
    }

    #[test]
    fn tool_call_limit_stops_without_auto_recovery_and_explicit_recovery_keeps_the_score() {
        let call =
            |id: String, name: &str, args: serde_json::Value| {
                crate::tests::completion(&json!({"role":"assistant","tool_calls":[{
                "id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}
            }]}).to_string(), "tool_calls")
            };
        let mut responses = vec![call(
            "setup".into(),
            "add_track",
            json!({"name":"Kit","kind":"drum"}),
        )];
        responses.extend((0..100).map(|id| call(format!("read-{id}"), "capabilities", json!({}))));
        let (url, seen) = crate::tests::mock_server(responses);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            max_tool_calls: Some(100),
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("1");
        let (sender, receiver) = mpsc::channel();
        let cancel = AtomicBool::new(false);
        let mut attempts = 0;
        let result = recover_candidate(
            &assignment(),
            &cancel,
            &sender,
            |_, _| true,
            |candidate| {
                attempts += 1;
                compose(
                    candidate,
                    prefs.clone(),
                    Settings::default(),
                    folder.clone(),
                    None,
                    &cancel,
                    &sender,
                    &[],
                )
            },
        );
        let error = result
            .err()
            .expect("the configured limit must stop this candidate");
        assert_eq!(crate::tool_calls::limit_from_error(&error), Some(100));
        assert_eq!(attempts, 1);
        assert_eq!(seen.lock().unwrap().len(), 101);
        assert!(
            !receiver
                .try_iter()
                .any(|event| matches!(event, JobEvent::Recovering(..)))
        );
        let mut restored = Session::new(SessionOptions::headless()).unwrap();
        restored
            .open(&folder.join("checkpoint/checkpoint.auris"))
            .unwrap();
        assert_eq!(restored.project().tracks.len(), 1);
        drop(restored);

        let (url, _) = crate::tests::mock_server(vec![
            call(
                "continue".into(),
                "generate_phrase",
                json!({"track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}),
            ),
            crate::tests::completion(r#"{"role":"assistant","content":"Finished"}"#, "stop"),
        ]);
        let mut candidate = assignment();
        candidate.error = Some(error);
        let prefs = AgentPreferences {
            url,
            max_tool_calls: Some(250),
            ..prefs
        };
        let recovered = compose(
            &candidate,
            prefs,
            Settings::default(),
            folder.clone(),
            None,
            &cancel,
            &sender,
            &[],
        )
        .unwrap();
        assert!(recovered.audio.peak() > 1e-8);
        let mut restored = Session::new(SessionOptions::headless()).unwrap();
        restored.open(&folder.join("song/song.auris")).unwrap();
        assert_eq!(restored.project().tracks.len(), 1);
    }

    #[test]
    fn composer_creates_persistent_audition_through_real_session_tools() {
        let call = |id: &str, name: &str, arguments: serde_json::Value| {
            json!({"role":"assistant","tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}).to_string()
        };
        let record = call(
            "record-1",
            "production_record",
            json!({"operation":{"kind":"enter_phase","phase":"score","reason":"new song","writing_order":"rhythm_first"}}),
        );
        let setup = call("setup-1", "add_track", json!({"name":"Kit","kind":"drum"}));
        let status = call(
            "status-1",
            "report_status",
            json!({"status":"ドラムの基本パターンを
書いています"}),
        );
        let phrase = call(
            "phrase-1",
            "generate_phrase",
            json!({"track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}),
        );
        let inspect = call(
            "inspect-1",
            "inspect_audio",
            json!({"start_bar":1,"bars":2}),
        );
        let answer = json!({"role":"assistant","content":"Finished"}).to_string();
        let (url, seen) = crate::tests::mock_server(vec![
            crate::tests::completion(&record, "tool_calls"),
            crate::tests::completion(&setup, "tool_calls"),
            crate::tests::completion(&status, "tool_calls"),
            crate::tests::completion(&phrase, "tool_calls"),
            crate::tests::completion(&inspect, "tool_calls"),
            crate::tests::completion(&answer, "stop"),
            // A two-bar drum sketch with no sections or roles is sent back twice for its
            // structure, then kept as it is.
            crate::tests::completion(&answer, "stop"),
            crate::tests::completion(&answer, "stop"),
        ]);
        let prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        let dir = tempfile::tempdir().unwrap();
        let (sender, receiver) = mpsc::channel();
        let result = compose(
            &assignment(),
            prefs,
            Settings::default(),
            dir.path().join("1"),
            None,
            &AtomicBool::new(false),
            &sender,
            &[],
        )
        .unwrap();
        assert!(result.audio.peak() > 1e-8);
        assert!(dir.path().join("1/song/song.auris").is_file());
        assert_eq!(seen.lock().unwrap().len(), 8);
        assert!(seen.lock().unwrap()[1].contains("worker_lifetime"));
        assert!(seen.lock().unwrap()[6].contains("roles_registered"));
        let events: Vec<_> = receiver.try_iter().collect();
        let completed: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                JobEvent::Tool(_, name, Some(ok)) => Some((name.as_str(), *ok)),
                _ => None,
            })
            .collect();
        assert_eq!(
            completed,
            vec![
                ("production_record", true),
                ("add_track", true),
                ("report_status", true),
                ("generate_phrase", true),
                ("inspect_audio", true)
            ]
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, JobEvent::Progress(_, stage) if stage == "compose"))
        );
        // The composer's own line reaches the card as one line, and is not mistaken for a stage.
        let statuses: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                JobEvent::Status(_, status) => Some(status.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(statuses, ["ドラムの基本パターンを 書いています"]);
    }

    #[test]
    fn the_card_shows_the_composers_words_only_while_it_composes() {
        let mut snapshot = Snapshot {
            threads: Vec::new(),
            document: Document::default(),
            candidates: BTreeMap::new(),
            previews: BTreeMap::new(),
            preview_errors: BTreeMap::new(),
            progress: BTreeMap::new(),
            status: BTreeMap::new(),
            planning: false,
            preferences: Preferences::default(),
            error: None,
            working: BTreeSet::new(),
            handled: 0,
        };
        snapshot.progress.insert(1, "melody".into());
        assert_eq!(snapshot.status_line(1), None);
        snapshot.status.insert(1, "サビの歌詞を当てています".into());
        assert_eq!(snapshot.status_line(1), Some("サビの歌詞を当てています"));
        for host in [
            "planning",
            "render_wait",
            "render",
            "stopping",
            "recovering",
        ] {
            snapshot.progress.insert(1, host.into());
            assert_eq!(snapshot.status_line(1), None, "{host}");
        }
        snapshot.progress.remove(&1);
        assert_eq!(snapshot.status_line(1), None);
    }
    #[test]
    fn cancellation_prevents_model_work_and_output() {
        let dir = tempfile::tempdir().unwrap();
        let (sender, _) = mpsc::channel();
        let result = compose(
            &assignment(),
            AgentPreferences::default(),
            Settings::default(),
            dir.path().join("1"),
            None,
            &AtomicBool::new(true),
            &sender,
            &[],
        );
        assert!(result.is_err());
        assert!(!dir.path().join("1").exists());
    }
    #[test]
    fn child_cannot_export_to_an_arbitrary_path() {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let error = execute(
            &mut session,
            json!({"action":"render","path":"untrusted.wav"}),
            None,
            &Settings::default(),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(error.contains("not available"));
    }
    #[test]
    fn clarification_does_not_dispatch_composers() {
        assert!(
            decision(r#"{"reply":"歌ものですか？","directions":[]}"#)
                .unwrap()
                .directions
                .is_empty()
        );
    }
    #[test]
    fn a_question_can_come_with_a_choice_card_and_an_unanswerable_one_is_refused() {
        let answer = decision(r#"{"reply":"まず雰囲気を。","directions":[],"choices":{"question":"どんな雰囲気？","options":[{"label":"明るい"},{"label":"切ない","detail":"短調寄り"}],"multiple":false}}"#).unwrap();
        let card = answer.choices.unwrap();
        assert_eq!(card.options[1].detail.as_deref(), Some("短調寄り"));
        assert!(
            decision(r#"{"reply":"ok","directions":[],"choices":null}"#)
                .unwrap()
                .choices
                .is_none()
        );
        let single = r#"{"reply":"?","directions":[],"choices":{"question":"Which?","options":[{"label":"Only"}]}}"#;
        assert!(decision(single).err().unwrap().contains("choices"));
    }

    #[test]
    fn a_numbered_list_becomes_a_card_only_when_it_could_be_answered() {
        let (reply, card) =
            numbered_choices("いいですね。\n１．サビを足す\n２．ベースを動かす\nどれにしますか？")
                .unwrap();
        assert_eq!(reply, "いいですね。\nどれにしますか？");
        assert_eq!(card.question, "どれにしますか？");
        assert_eq!(card.options[1].label, "ベースを動かす");
        assert!(numbered_choices("1. Only one\nWhich?").is_none());
        assert!(numbered_choices("No list here.").is_none());
        assert!(numbered_choices("1. Same\n2. Same\nWhich?").is_none());
    }

    #[test]
    fn single_direction_is_accepted() {
        let answer = decision(r#"{"reply":"作ります","directions":[{"title":"A","description":"Piano","instructions":"Compose"}]}"#).unwrap();
        assert_eq!(answer.directions.len(), 1);
    }

    #[test]
    fn two_directions_are_accepted() {
        let directions = vec![assignment().direction; 2];
        let answer =
            decision(&json!({"reply":"Compare these", "directions":directions}).to_string())
                .unwrap();
        assert_eq!(answer.directions.len(), 2);
    }

    #[test]
    fn oversized_batch_is_rejected() {
        let directions = vec![assignment().direction; 4];
        assert!(
            decision(&json!({"reply":"Compare these", "directions":directions}).to_string())
                .is_err()
        );
    }

    #[test]
    fn a_long_conversation_is_summarized_once_while_the_director_is_idle() {
        use auris_session::workshop::digest_policy::*;
        let summary = "The user wants a calm piano piece in D minor.";
        let answer = json!({"role":"assistant","content":summary}).to_string();
        let (url, requests) =
            crate::tests::mock_server(vec![crate::tests::completion(&answer, "stop")]);
        let directory = tempfile::tempdir().unwrap();
        let (sender, _) = mpsc::channel();
        let mut owner = Owner::new(
            Store::new(directory.path().into()),
            Settings::default(),
            sender,
        );
        owner.snapshot.preferences.enabled = false;
        owner.prefs = AgentPreferences {
            provider: "openai".into(),
            model: "mock".into(),
            url,
            ..Default::default()
        };
        for index in 0..=CONVERSATION_MESSAGES {
            owner.snapshot.document.messages.push(Message {
                choices: None,
                references: vec![],
                attachments: Vec::new(),
                user: index % 2 == 0,
                text: format!("message {index}"),
            });
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while owner.snapshot.document.digest.is_empty() {
            owner.digest();
            assert!(Instant::now() < deadline, "the summary never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(owner.snapshot.document.digest, summary);
        assert_eq!(
            owner.snapshot.document.model_messages().len(),
            CONVERSATION_KEEP
        );
        assert_eq!(
            owner.snapshot.document.messages.len(),
            CONVERSATION_MESSAGES + 1
        );
        owner.digest();
        assert!(owner.conversation_digest.is_none());
        let requests = requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0].contains("message 0"));
        assert!(!requests[0].contains(&format!("message {CONVERSATION_MESSAGES}")));
        let stored = Store::new(directory.path().into()).load().unwrap();
        assert_eq!(stored.digest, summary);
        let assignment = composition_assignment(&assignment(), &owner.snapshot.document);
        let assignment: serde_json::Value = serde_json::from_str(&assignment).unwrap();
        assert_eq!(assignment["conversation_summary"], summary);
        assert_eq!(
            assignment["conversation"].as_array().unwrap().len(),
            CONVERSATION_KEEP
        );
    }
}
