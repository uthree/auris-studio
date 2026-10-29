//! Conversational song director and bounded, independently cancellable composers.

use auris_session::workshop::{
    Candidate, Direction, Document, Message, Preferences, Round, Selection, Store,
};
use auris_session::{AgentPreferences, Session, SessionOptions, Settings, SongCandidate, SongIdea};
use serde_json::json;
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

mod preview;

/// An immutable audition of an unfinished candidate, separate from adoptable music.
#[derive(Clone)]
pub struct Preview {
    /// Content revision captured before rendering.
    pub revision: u64,
    /// Rendered audio; never mutated while the user is listening.
    pub audio: Arc<auris_session::prelude::AudioBuffer>,
}

/// Automatic restarts allowed per candidate and explicit recovery request.
pub const AUTO_RECOVERY_LIMIT: u32 = 3;

/// Commands accepted by the background workshop owner.
pub enum Command {
    /// Submit one natural-language message with model settings captured at submission.
    Send(String, Box<AgentPreferences>),
    /// Choose an exact completed candidate without closing the conversation.
    Select(u64, Box<AgentPreferences>, String),
    /// Retry only this candidate in a new artifact directory.
    Retry(u64),
    /// Continue an unfinished candidate from its checkpoint with a fresh recovery budget.
    Recover(u64, Box<AgentPreferences>),
    /// Load auditions for an older comparison without retaining every historical WAV.
    View(usize),
    /// Stop one candidate, or all work when absent.
    Stop(Option<u64>),
    /// Persist an unsent draft before closing.
    Draft(String),
    /// Enable or disable cross-project preference evidence.
    Memory(bool),
    /// Forget all recorded preferences.
    Forget,
    /// Remove one incorrect or no-longer-relevant preference observation.
    ForgetChoice(String, u64),
}

/// Full UI snapshot, containing immutable shared audition buffers.
pub struct Snapshot {
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
    /// The director is responding.
    pub planning: bool,
    /// Preference evidence the user may inspect or clear.
    pub preferences: Preferences,
    /// Storage/provider/runtime failure, never silently swallowed.
    pub error: Option<String>,
}

/// One background owner; dropping it cancels all child workers.
pub struct Workshop {
    sender: mpsc::Sender<Command>,
    receiver: mpsc::Receiver<Snapshot>,
    stopped: Arc<AtomicBool>,
}

impl Workshop {
    /// Start or restore a project-scoped conversation off the UI thread.
    pub fn start(folder: PathBuf, unsaved: bool, settings: Settings) -> Result<Self, String> {
        let (sender, commands) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let cancelled = stopped.clone();
        std::thread::Builder::new()
            .name("auris-workshop".into())
            .spawn(move || {
                let result = (|| {
                    let conversations = if unsaved {
                        auris_session::conversations::ConversationStore::for_unsaved_document(
                            &folder,
                        )?
                    } else {
                        auris_session::conversations::ConversationStore::for_project(&folder)
                    };
                    let store = Store::new(conversations.root().into());
                    let _lease = store.lease()?;
                    // Do not overwrite an unreadable history with a fresh default.
                    store.load()?;
                    let mut owner = Owner::new(store, settings, events.clone());
                    owner.run(commands, cancelled);
                    Ok::<(), String>(())
                })();
                if let Err(error) = result {
                    let _ = events.send(Snapshot {
                        document: Document::default(),
                        candidates: BTreeMap::new(),
                        previews: BTreeMap::new(),
                        preview_errors: BTreeMap::new(),
                        progress: BTreeMap::new(),
                        planning: false,
                        preferences: Preferences::default(),
                        error: Some(error),
                    });
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Self {
            sender,
            receiver,
            stopped,
        })
    }
    /// Queue a command without blocking the window.
    pub fn send(&self, command: Command) -> bool {
        self.sender.send(command).is_ok()
    }
    /// Drain available snapshots; callers should keep the newest.
    pub fn poll(&self) -> Option<Snapshot> {
        self.receiver.try_iter().last()
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
                ConversationContent::User {
                    text: message.text.clone(),
                    attachments: vec![],
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
    conversation.transcript.draft = document.draft;
    store.save(&conversation.info.id, &conversation.transcript)?;
    memory.save(&store.memory_path(&conversation.info.id)?)?;
    Ok((store, conversation.info.id))
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Decision {
    reply: String,
    directions: Vec<Direction>,
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
        || !matches!(value.directions.len(), 0 | 3)
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
    Ok(value)
}

enum JobEvent {
    Progress(u64, String),
    Preview(u64, Result<Preview, String>),
    Recovering(u64, u32, String),
    Done(u64, Result<Arc<SongCandidate>, String>),
}
struct Owner {
    store: Store,
    settings: Settings,
    snapshot: Snapshot,
    events: mpsc::Sender<Snapshot>,
    planner: Option<crate::song_ideas::SongPlanner>,
    planned_parent: Option<u64>,
    selection_followup: bool,
    prefs: AgentPreferences,
    jobs: BTreeMap<u64, Arc<AtomicBool>>,
    job_sender: mpsc::Sender<JobEvent>,
    job_receiver: mpsc::Receiver<JobEvent>,
    handles: Vec<std::thread::JoinHandle<()>>,
}

impl Owner {
    fn new(store: Store, settings: Settings, events: mpsc::Sender<Snapshot>) -> Self {
        let (job_sender, job_receiver) = mpsc::channel();
        let mut error = None;
        let mut document = store.load().unwrap_or_else(|e| {
            error = Some(e);
            Document::default()
        });
        for candidate in document.rounds.iter_mut().flat_map(|r| &mut r.candidates) {
            if !candidate.ready && candidate.error.is_none() {
                candidate.error = Some("Production was interrupted; retry this candidate".into());
            }
        }
        let preferences = auris_session::workshop::load_preferences().unwrap_or_else(|e| {
            error = Some(e);
            Preferences::default()
        });
        let mut candidates = BTreeMap::new();
        for candidate in document
            .rounds
            .iter()
            .flat_map(|r| &r.candidates)
            .filter(|c| {
                c.ready
                    && (Some(c.id) == document.selected
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
            prefs: settings.agent.clone(),
            store,
            settings,
            events,
            planner: None,
            planned_parent: None,
            selection_followup: false,
            jobs: BTreeMap::new(),
            job_sender,
            job_receiver,
            handles: Vec::new(),
            snapshot: Snapshot {
                document,
                candidates,
                previews: BTreeMap::new(),
                preview_errors: BTreeMap::new(),
                progress: BTreeMap::new(),
                planning: false,
                preferences,
                error,
            },
        }
    }
    fn publish(&mut self, save: bool) {
        if save && let Err(e) = self.store.save(&self.snapshot.document) {
            self.snapshot.error = Some(e);
        }
        let _ = self.events.send(Snapshot {
            document: self.snapshot.document.clone(),
            candidates: self.snapshot.candidates.clone(),
            previews: self.snapshot.previews.clone(),
            preview_errors: self.snapshot.preview_errors.clone(),
            progress: self.snapshot.progress.clone(),
            planning: self.planner.is_some(),
            preferences: self.snapshot.preferences.clone(),
            error: self.snapshot.error.clone(),
        });
    }
    fn run(&mut self, commands: mpsc::Receiver<Command>, stopped: Arc<AtomicBool>) {
        self.publish(false);
        while !stopped.load(Ordering::Acquire) {
            match commands.recv_timeout(Duration::from_millis(25)) {
                Ok(command) => {
                    self.command(command);
                    self.publish(true);
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(answer) = self.planner.as_mut().and_then(|p| p.poll_text()) {
                self.planner = None;
                match answer.and_then(|text| decision(&text)) {
                    Err(e) => self.snapshot.error = Some(e),
                    Ok(answer) => {
                        self.snapshot.document.messages.push(Message {
                            user: false,
                            text: answer.reply,
                        });
                        if !self.selection_followup && !answer.directions.is_empty() {
                            let mut candidates = Vec::new();
                            for direction in answer.directions {
                                self.snapshot.document.next_id += 1;
                                candidates.push(Candidate {
                                    id: self.snapshot.document.next_id,
                                    parent: self.planned_parent,
                                    direction,
                                    ready: false,
                                    error: None,
                                    recoveries: 0,
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
                            candidate.error = Some(error);
                        }
                        self.snapshot.progress.insert(id, "recovering".into());
                        self.publish(true);
                    }
                    JobEvent::Progress(id, stage) if self.jobs.contains_key(&id) => {
                        self.snapshot.progress.insert(id, stage);
                        self.publish(false);
                    }
                    JobEvent::Done(id, result) => {
                        let cancelled = self
                            .jobs
                            .remove(&id)
                            .is_none_or(|c| c.load(Ordering::Acquire));
                        self.snapshot.progress.remove(&id);
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
                        self.publish(true);
                    }
                    _ => {}
                }
            }
        }
        for cancel in self.jobs.values() {
            cancel.store(true, Ordering::Release);
        }
        self.planner = None;
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
        // Draft commands sent immediately before Drop must still be durable.
        for command in commands.try_iter() {
            if let Command::Draft(draft) = command {
                self.snapshot.document.draft = draft;
            }
        }
        self.publish(true);
    }
    fn command(&mut self, command: Command) {
        match command {
            Command::Send(text, prefs) if self.planner.is_none() && self.jobs.is_empty() => {
                if text.trim().is_empty() || text.chars().count() > 16000 {
                    self.snapshot.error = Some("Enter 1..16000 characters".into());
                    return;
                }
                self.prefs = *prefs;
                self.selection_followup = false;
                self.planned_parent = self.snapshot.document.selected;
                self.snapshot.error = None;
                match auris_session::workshop::load_preferences() {
                    Ok(preferences) => self.snapshot.preferences = preferences,
                    Err(e) => {
                        self.snapshot.error = Some(e);
                        return;
                    }
                }
                self.snapshot.document.draft.clear();
                self.snapshot
                    .document
                    .messages
                    .push(Message { user: true, text });
                let selected = self
                    .snapshot
                    .document
                    .rounds
                    .iter()
                    .flat_map(|r| &r.candidates)
                    .find(|c| Some(c.id) == self.snapshot.document.selected);
                let voices = auris_session::library::voices_with_settings(&self.settings);
                let prompt = json!({"conversation": self.snapshot.document.messages, "selected": selected, "selected_score":self.snapshot.document.selected.and_then(|id| self.snapshot.candidates.get(&id)).map(|c| c.workshop_outline()),
                    "preference_evidence": self.snapshot.preferences.context(), "installed_voices": voices}).to_string();
                match crate::song_ideas::SongPlanner::conversation(self.prefs.clone(), prompt) {
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
                    "conversation":self.snapshot.document.messages,
                    "selected":selected,
                    "selected_score":self.snapshot.candidates[&id].workshop_outline(),
                    "preference_recorded":recorded,
                    "instruction":"Acknowledge the selected song and propose 3 to 5 concrete, distinct arrangement or improvement options in a numbered list in reply, grounded in this song and the conversation. Ask which option to pursue and invite a custom request. Mention preference recording only if preference_recorded is true. Return an empty directions array; selection requests suggestions, not another production batch. Do not claim to have listened."
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
            Command::Retry(id)
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
                    self.snapshot.document.next_id += 1;
                    candidate.id = self.snapshot.document.next_id;
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
            Command::Draft(draft) => self.snapshot.document.draft = draft,
            Command::View(index) => {
                if let Some(round) = self.snapshot.document.rounds.get(index) {
                    let ids: Vec<_> = round
                        .candidates
                        .iter()
                        .filter(|c| c.ready)
                        .map(|c| c.id)
                        .collect();
                    self.snapshot.candidates.retain(|id, _| {
                        ids.contains(id) || Some(*id) == self.snapshot.document.selected
                    });
                    self.snapshot.previews.retain(|id, _| {
                        self.jobs.contains_key(id) || round.candidates.iter().any(|c| c.id == *id)
                    });
                    self.snapshot.preview_errors.retain(|id, _| {
                        self.jobs.contains_key(id) || round.candidates.iter().any(|c| c.id == *id)
                    });
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
            Command::Memory(enabled) => {
                match auris_session::workshop::update_preferences(|p| p.enabled = enabled) {
                    Ok(p) => self.snapshot.preferences = p,
                    Err(e) => self.snapshot.error = Some(e),
                }
            }
            Command::Forget => {
                match auris_session::workshop::update_preferences(|p| p.selections.clear()) {
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
        candidate.direction.instructions = json!({"assignment":candidate.direction.instructions,"conversation":self.snapshot.document.messages}).to_string();
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

// Render and voice inference share a conservative process-wide gate; model turns remain parallel.
static AUDIO_WORK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn compose(
    candidate: &Candidate,
    mut prefs: AgentPreferences,
    settings: Settings,
    folder: PathBuf,
    parent: Option<PathBuf>,
    cancel: &AtomicBool,
    sender: &mpsc::Sender<JobEvent>,
) -> Result<SongCandidate, AttemptError> {
    if cancel.load(Ordering::Acquire) {
        return Err("Cancelled".into());
    }
    let mut session = Session::new(
        SessionOptions::headless()
            .with_balance(false)
            .with_plugin_paths(settings.plugin_paths.clone()),
    )
    .map_err(|e| e.to_string())?;
    if let Some(dictionary) = &settings.japanese_dictionary {
        session
            .set_japanese_dictionary(Some(dictionary))
            .map_err(|e| e.to_string())?;
    }
    if let Some(parent) = parent {
        let missing = session.open(&parent).map_err(|e| e.to_string())?;
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
    if checkpoint
        .try_exists()
        .map_err(|e| AttemptError::permanent(e.to_string()))?
    {
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
        deny: vec!["search_internet".into(), "edit_project.listen".into()],
    };
    prefs.system_prompt = format!(
        "{}\n{}",
        crate::system_prompt(&prefs),
        auris_toolbox::workshop::COMPOSER
    );
    let worker = crate::Worker::spawn(prefs.clone(), None, true, None)?;
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
    worker.send(
        &json!({"say": json!({"assignment":candidate.direction.instructions,
            "recovery":candidate.error.as_ref().map(|error| json!({
                "last_error":error,
                "instruction":"Inspect the restored score and current resources. Continue only unfinished work; do not duplicate successful edits. Correct the cause of the last failure before completing."
            }))}).to_string(),"policy":prefs.policy}).to_string(),
    )?;
    let deadline = Instant::now() + Duration::from_secs(900);
    let mut edits = 0;
    let mut denials = 0;
    let mut last_failure = String::new();
    let completion = loop {
        if cancel.load(Ordering::Acquire) {
            return Err("Cancelled".into());
        }
        if Instant::now() > deadline {
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
            "edit" => {
                edits += 1;
                if edits > 160 {
                    return Err("Candidate exceeded its tool budget".into());
                }
                let action = event["command"]["action"].as_str().unwrap_or_default();
                let stage = match action {
                    "edit_harmony" | "suggest_harmony" => "harmony",
                    "edit_notes" | "generate_phrase" => "melody",
                    "sing" | "set_singer_voice" => "sing",
                    "compose" | "add_track" => "compose",
                    _ => "arrange",
                };
                let _ = sender.send(JobEvent::Progress(candidate.id, stage.into()));
                let result = execute(&mut session, event["command"].clone(), &settings, cancel);
                if result.is_ok() && session.is_dirty() {
                    validate_protected(&session)?;
                    save_checkpoint(&mut session, &folder)?;
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
                break event["text"]
                    .as_str()
                    .unwrap_or_default()
                    .chars()
                    .take(2000)
                    .collect::<String>();
            }
            "error" => {
                return Err(event["message"]
                    .as_str()
                    .unwrap_or("Candidate failed")
                    .into());
            }
            "ended" => return Err("Candidate ended without completing".into()),
            _ => {}
        }
    };
    // Join before publishing a finished candidate so no late draft can replace its audition.
    drop(previews);
    session.end_agent_request();
    validate_protected(&session)?;
    if session.project() == &before {
        return Err(format!(
            "The composer did not make the requested music changes. {completion} {last_failure}"
        )
        .into());
    }
    if !last_failure.is_empty() {
        return Err(format!("The last tool failure remains unresolved: {last_failure}").into());
    }
    let _ = sender.send(JobEvent::Progress(candidate.id, "render_wait".into()));
    let _gate = audio_gate(cancel)?;
    let _ = sender.send(JobEvent::Progress(candidate.id, "render".into()));
    session
        .finish_workshop_candidate(&folder, idea(&candidate.direction), cancel)
        .map_err(Into::into)
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
    settings: &Settings,
    cancel: &AtomicBool,
) -> Result<String, String> {
    let action = command["action"].as_str().unwrap_or_default();
    // File exports/imports, playback, nested jobs and network tools are not child capabilities.
    if matches!(
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
    ) {
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
    if vocal {
        let _gate = audio_gate(cancel)?;
        let mut job = session.begin_agent_vocal(command, settings.clone())?;
        loop {
            let result = job.run(cancel)?;
            match session.continue_agent_vocal(result)? {
                auris_session::live_agent::VocalStep::Work(next) => job = *next,
                auris_session::live_agent::VocalStep::Done(text) => return Ok(text),
            }
        }
    }
    session.agent_command_with_plugin_paths(command, &settings.plugin_paths)
}

#[cfg(test)]
mod tests {
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
    fn editor_handoff_keeps_readable_turns_and_provider_memory() {
        let dir = tempfile::tempdir().unwrap();
        let document = Document {
            messages: vec![
                Message {
                    user: true,
                    text: "Keep the melody".into(),
                },
                Message {
                    user: false,
                    text: "I will change only the accompaniment".into(),
                },
            ],
            draft: "Make the chorus brighter".into(),
            ..Default::default()
        };
        let (store, id) = handoff(dir.path().into(), false, document).unwrap();
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
    fn assignment() -> Candidate {
        Candidate {
            id: 1,
            parent: None,
            ready: false,
            error: None,
            recoveries: 0,
            direction: Direction {
                title: "Test".into(),
                description: "A short instrumental".into(),
                instructions: "Create a short chiptune instrumental".into(),
                protected_tracks: vec![],
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
            &Settings::default(),
            &cancel,
        )
        .unwrap();
        execute(&mut session, json!({"action":"generate_phrase","track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}), &Settings::default(), &cancel).unwrap();
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
        let reply = "Selected. 1. Add bass. 2. Vary the drums. 3. Build a chorus. Which next?";
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
            if snapshot
                .document
                .messages
                .last()
                .is_some_and(|m| !m.user && m.text == reply)
            {
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
            &Settings::default(),
            &cancel,
        )
        .unwrap();
        execute(&mut session, json!({"action":"generate_phrase","track":1,"part":"drums","start_bar":1,"bars":2,"seed":42}), &Settings::default(), &cancel).unwrap();
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
        )
        .unwrap();
        assert!(result.audio.peak() > 1e-8);
        let mut restored = Session::new(SessionOptions::headless()).unwrap();
        restored.open(&folder.join("song/song.auris")).unwrap();
        assert_eq!(restored.project().tracks, original);
        assert!(seen.lock().unwrap()[0].contains("Connection interrupted after writing notes"));
    }

    #[test]
    fn composer_creates_persistent_audition_through_real_session_tools() {
        let call = |id: &str, name: &str, arguments: serde_json::Value| {
            json!({"role":"assistant","tool_calls":[{"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}}]}).to_string()
        };
        let record = call(
            "record-1",
            "production_record",
            json!({"operation":{"kind":"select_workflow","workflow":"composition","writing_order":"rhythm_first"}}),
        );
        let setup = call("setup-1", "add_track", json!({"name":"Kit","kind":"drum"}));
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
            crate::tests::completion(&phrase, "tool_calls"),
            crate::tests::completion(&inspect, "tool_calls"),
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
        )
        .unwrap();
        assert!(result.audio.peak() > 1e-8);
        assert!(dir.path().join("1/song/song.auris").is_file());
        assert_eq!(seen.lock().unwrap().len(), 5);
        assert!(seen.lock().unwrap()[1].contains("worker_lifetime"));
        assert!(
            receiver
                .try_iter()
                .any(|e| matches!(e, JobEvent::Progress(_, stage) if stage == "compose"))
        );
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
    fn incomplete_comparison_is_rejected() {
        assert!(decision(r#"{"reply":"作ります","directions":[{"title":"A","description":"Piano","instructions":"Compose"}]}"#).is_err());
    }
}
