//! Durable new-song conversations and evidence-based preference retrieval.
//! All methods that touch storage must run away from the UI thread.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One readable turn, kept independently from provider memory.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Message {
    /// Archived user references, resolved for display and explicit reuse by the workshop.
    #[serde(default)]
    pub attachments: Vec<PathBuf>,
    /// Whether the user authored this message.
    pub user: bool,
    /// Original text, including authored lyric line breaks.
    pub text: String,
    /// Exact symbolic references captured on submission, independent of later file edits.
    #[serde(default)]
    pub references: Vec<Reference>,
    /// Fixed answers the director offered with this reply, drawn as a card under it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choices: Option<crate::choices::ChoiceCard>,
}

/// One readable attachment and its bounded model-facing snapshot.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Reference {
    /// Explicitly attached local file, retained for display and editor handoff.
    pub path: PathBuf,
    /// Decoded note, chord or instrument data; never an audio-listening claim.
    pub text: String,
}

/// One independent candidate assignment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Direction {
    /// Short, user-facing name.
    pub title: String,
    /// Audible distinction from the other alternatives.
    pub description: String,
    /// Complete instructions for the isolated composer.
    pub instructions: String,
    /// Exact parent track IDs whose note clips must remain unchanged.
    #[serde(default)]
    pub protected_tracks: Vec<u64>,
}

/// Persisted candidate identity; IDs also name private artifact directories.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Candidate {
    /// Stable local ID.
    pub id: u64,
    /// Exact source candidate, when continuing an existing song.
    pub parent: Option<u64>,
    /// Composition assignment.
    pub direction: Direction,
    /// True only after document, assets and audition have been saved.
    pub ready: bool,
    /// Failure retained across reopening.
    pub error: Option<String>,
    /// Automatic recoveries consumed since the last explicit recovery request.
    #[serde(default)]
    pub recoveries: u32,
    /// Tool usage across all attempts, retained with the conversation.
    #[serde(default)]
    pub tools: std::collections::BTreeMap<String, ToolUsage>,
    /// Explicit human feedback for this exact finished candidate.
    #[serde(default)]
    pub feedback: Option<Feedback>,
}

/// A user's rating and optional listening impressions, scoped to one conversation.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Feedback {
    /// Whole-star rating, from one through five.
    pub rating: u8,
    /// User-authored impressions, up to 2,000 Unicode characters.
    pub comment: String,
}

impl Feedback {
    /// Validate the rating and comment before changing durable conversation state.
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=5).contains(&self.rating) {
            return Err("Choose a rating from 1 to 5".into());
        }
        if self.comment.chars().count() > 2000 {
            return Err("Feedback must contain at most 2000 characters".into());
        }
        Ok(())
    }
}

/// Execution counts for one tool, including unfinished and interrupted calls.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ToolUsage {
    /// Calls observed starting.
    pub total: u64,
    /// Calls that returned successfully.
    pub succeeded: u64,
    /// Calls that returned an error.
    pub failed: u64,
    /// Calls interrupted before a result was received.
    pub interrupted: u64,
}

impl ToolUsage {
    /// Calls still awaiting a result.
    pub fn running(&self) -> u64 {
        self.total
            .saturating_sub(self.succeeded + self.failed + self.interrupted)
    }

    /// Finish pending observations without treating cancellation as a tool failure.
    pub fn interrupt(&mut self) {
        self.interrupted += self.running();
    }
}

/// A comparison is retained even after another round is generated.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Round {
    /// Transcript position immediately after the director's response.
    pub after_message: usize,
    /// Alternatives in stable display order.
    pub candidates: Vec<Candidate>,
}

/// Provider-independent workshop state, separate from score undo history.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Document {
    /// Host-assigned workshop thread identity; older histories use `default` when loaded.
    pub thread_id: String,
    /// Optional user-assigned title, otherwise derived from the first request.
    pub title: String,
    /// Hidden from the ordinary sidebar while remaining recoverable.
    pub archived: bool,
    /// Monotonic identity allocator.
    pub next_id: u64,
    /// Full readable conversation.
    pub messages: Vec<Message>,
    /// Comparison history and parent links.
    pub rounds: Vec<Round>,
    /// Current musical baseline.
    pub selected: Option<u64>,
    /// Unsent composer contents.
    pub draft: String,
    /// Unsent local references, restored beside the draft message.
    pub draft_attachments: Vec<PathBuf>,
}

impl Document {
    /// Baseline for further edits: an explicit selection, or the latest single finished song.
    /// Failed rounds do not replace a usable baseline, and comparisons require a user choice.
    /// This does not change the explicit selection or record preference evidence.
    pub fn continuation_candidate(&self) -> Option<&Candidate> {
        if let Some(selected) = self.selected {
            return self
                .rounds
                .iter()
                .flat_map(|r| &r.candidates)
                .find(|c| c.id == selected && c.ready);
        }
        let round = self
            .rounds
            .iter()
            .rev()
            .find(|r| r.candidates.iter().any(|c| c.ready))?;
        match round.candidates.as_slice() {
            [candidate] => Some(candidate),
            _ => None,
        }
    }

    /// Compact sidebar title without treating model output as user metadata.
    pub fn sidebar_title(&self) -> String {
        if !self.title.is_empty() {
            return self.title.clone();
        }
        self.messages
            .iter()
            .find(|m| m.user)
            .map_or_else(String::new, |m| {
                m.text
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(60)
                    .collect()
            })
    }
    /// Rename this conversation without changing its identity or generated candidates.
    pub fn rename(&mut self, title: &str) -> Result<(), String> {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 80 {
            return Err("Conversation titles must contain 1 to 80 characters".into());
        }
        self.title = title.to_owned();
        Ok(())
    }
    /// Record or replace feedback only for a completed candidate in this conversation.
    pub fn record_feedback(&mut self, id: u64, feedback: Feedback) -> Result<(), String> {
        feedback.validate()?;
        let candidate = self
            .rounds
            .iter_mut()
            .flat_map(|r| &mut r.candidates)
            .find(|c| c.id == id && c.ready)
            .ok_or("Feedback requires a completed candidate")?;
        candidate.feedback = Some(feedback);
        Ok(())
    }
}

/// A choice is evidence of a relative preference, never a dislike of every other song.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Selection {
    /// Conversation identity, preventing collisions between projects.
    pub conversation: String,
    /// Candidate identity within the conversation.
    pub candidate: u64,
    /// Stable comparison identity; different rounds may reuse the same titles.
    #[serde(default)]
    pub round: u64,
    /// User's request and context, not an inferred personality trait.
    pub context: String,
    /// All alternatives actually offered.
    pub alternatives: Vec<Direction>,
    /// Selected alternative's position.
    pub chosen: usize,
}

/// Local preference evidence. Explicit current instructions always take precedence.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    /// Whether to record and retrieve choices.
    pub enabled: bool,
    /// Bounded recent evidence, with original context.
    pub selections: Vec<Selection>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            enabled: true,
            selections: Vec::new(),
        }
    }
}

impl Preferences {
    /// Correct or remove an observation, rejecting stale edits from another window.
    /// Candidate identities and the offered alternatives remain unchanged. A correction
    /// becomes the newest evidence so it is included in the next bounded retrieval.
    pub fn revise_selection(
        &mut self,
        original: &Selection,
        correction: Option<(String, usize)>,
    ) -> Result<(), String> {
        let index = self
            .selections
            .iter()
            .position(|s| s == original)
            .ok_or_else(|| {
                "This preference changed. Refresh the history and try again.".to_owned()
            })?;
        if let Some((context, chosen)) = correction {
            if context.trim().is_empty() || chosen >= original.alternatives.len() {
                return Err("Enter a request and choose one of the offered alternatives.".into());
            }
            let mut revised = self.selections.remove(index);
            revised.context = context;
            revised.chosen = chosen;
            self.selections.push(revised);
        } else {
            self.selections.remove(index);
        }
        Ok(())
    }

    /// Record a choice once; selecting another alternative replaces that comparison's vote.
    pub fn record(&mut self, selection: Selection) {
        if !self.enabled {
            return;
        }
        self.selections.retain(|old| {
            !(old.conversation == selection.conversation && old.round == selection.round)
        });
        self.selections.push(selection);
        if self.selections.len() > 100 {
            self.selections.remove(0);
        }
    }

    /// Bounded attributed evidence for a director, rather than an ungrounded preference summary.
    pub fn context(&self) -> String {
        if !self.enabled {
            return String::new();
        }
        serde_json::to_string(&self.selections.iter().rev().take(8).map(|s| serde_json::json!({
            "context":s.context, "chosen":s.chosen,
            "alternatives":s.alternatives.iter().map(|d| serde_json::json!({"title":d.title,"description":d.description})).collect::<Vec<_>>()
        })).collect::<Vec<_>>())
            .unwrap_or_default()
    }
}

/// Song-conversation storage. The application keeps one library, [`Store::library`], because the
/// workshop starts songs rather than editing one: it opens before any project exists, so a store
/// keyed by the open document would begin empty on every launch.
#[derive(Clone)]
pub struct Store {
    root: PathBuf,
    library_root: PathBuf,
    thread_id: String,
}

/// A song conversation shown in the workshop sidebar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThreadInfo {
    /// Opaque host-assigned directory identity.
    pub id: String,
    /// User title or first request; empty means an untitled conversation.
    pub title: String,
    /// Whether the conversation is in the archive.
    pub archived: bool,
    /// Last saved time in seconds since the Unix epoch.
    pub updated: u64,
}

impl Store {
    /// Hold one process-wide workshop owner until its worker exits.
    pub fn lease(&self) -> Result<std::fs::File, String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("workshop.lock"))
            .map_err(|e| e.to_string())?;
        file.try_lock()
            .map_err(|_| "This song conversation is already open in another window".to_string())?;
        Ok(file)
    }

    /// Copy closed workshop history and exact artifacts to another, absent, location.
    pub fn copy_to(&self, destination: &Self) -> Result<(), String> {
        if self.root == destination.root || !self.root.exists() {
            return Ok(());
        }
        let _lease = self.lease()?;
        let mut thread_leases = Vec::new();
        if self.thread_id == "default" {
            for thread in self.threads()? {
                if thread.id != "default" {
                    thread_leases.push(self.thread(&thread.id)?.lease()?);
                }
            }
        }
        if destination.root.exists() {
            if let (Ok(source), Ok(target)) = (
                std::fs::read(self.root.join("conversation.json")),
                std::fs::read(destination.root.join("conversation.json")),
            ) && source == target
                && self.same_thread_history(destination)?
            {
                return Ok(());
            }
            return Err("Destination contains a different workshop conversation".into());
        }
        let parent = destination.root.parent().ok_or("Missing workshop parent")?;
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let staging = tempfile::tempdir_in(parent).map_err(|e| e.to_string())?;
        copy_tree(&self.root, staging.path())?;
        std::fs::rename(staging.path(), &destination.root).map_err(|e| e.to_string())?;
        Ok(())
    }
    fn same_thread_history(&self, other: &Self) -> Result<bool, String> {
        let mut source = self
            .threads()?
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>();
        let mut target = other
            .threads()?
            .into_iter()
            .map(|t| t.id)
            .collect::<Vec<_>>();
        source.sort();
        target.sort();
        if source != target {
            return Ok(false);
        }
        for id in source {
            let a = std::fs::read(self.thread(&id)?.root.join("conversation.json"))
                .map_err(|e| e.to_string())?;
            let b = std::fs::read(other.thread(&id)?.root.join("conversation.json"))
                .map_err(|e| e.to_string())?;
            if a != b {
                return Ok(false);
            }
        }
        let a: Option<String> = read_or_default(&self.library_root.join("active-thread.json"))?;
        let b: Option<String> = read_or_default(&other.library_root.join("active-thread.json"))?;
        Ok(a == b)
    }
    /// The application's song-conversation library, in the configuration directory.
    ///
    /// Every workshop window opens this one library, whichever document is open behind it, so
    /// the sidebar lists every earlier conversation.
    pub fn library() -> Self {
        Self::new(crate::config_dir())
    }
    /// Use a host-resolved conversation directory, never a model-provided path.
    pub fn new(root: PathBuf) -> Self {
        let root = root.join("workshop");
        Self {
            library_root: root.clone(),
            root,
            thread_id: "default".into(),
        }
    }
    /// Open an exact thread without interpreting its ID as a filesystem path.
    pub fn thread(&self, id: &str) -> Result<Self, String> {
        if id != "default"
            && std::fs::symlink_metadata(self.library_root.join("threads"))
                .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err("Song conversations may not be symbolic links".into());
        }
        if id != "default"
            && (!id.starts_with("song-")
                || id.len() > 96
                || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
        {
            return Err("Invalid song conversation identity".into());
        }
        let root = if id == "default" {
            self.library_root.clone()
        } else {
            self.library_root.join("threads").join(id)
        };
        if std::fs::symlink_metadata(&root).is_ok_and(|m| m.file_type().is_symlink()) {
            return Err("Song conversations may not be symbolic links".into());
        }
        Ok(Self {
            root,
            library_root: self.library_root.clone(),
            thread_id: id.into(),
        })
    }
    /// Restore the last active thread while retaining the original history in place.
    pub fn active_thread(&self) -> Result<Self, String> {
        let id: Option<String> = read_or_default(&self.library_root.join("active-thread.json"))?;
        let store = self.thread(id.as_deref().unwrap_or("default"))?;
        if store.thread_id != "default" && !store.root.join("conversation.json").is_file() {
            return Err("The active song conversation is missing".into());
        }
        Ok(store)
    }
    /// Remember a successfully opened thread.
    pub fn set_active_thread(&self) -> Result<(), String> {
        write(
            &self.library_root.join("active-thread.json"),
            &self.thread_id,
        )
    }
    /// Allocate an independent empty conversation without rewriting existing artifacts.
    pub fn create_thread(&self) -> Result<Self, String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos();
        let id = format!(
            "song-{time:x}-{:x}-{:x}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let store = self.thread(&id)?;
        std::fs::create_dir_all(self.library_root.join("threads")).map_err(|e| e.to_string())?;
        std::fs::create_dir(&store.root).map_err(|e| e.to_string())?;
        store.save(&Document::default())?;
        Ok(store)
    }
    /// List both ordinary and archived conversations, newest saved first.
    pub fn threads(&self) -> Result<Vec<ThreadInfo>, String> {
        let mut ids = vec!["default".to_owned()];
        match std::fs::read_dir(self.library_root.join("threads")) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(|e| e.to_string())?;
                    if entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                        ids.push(entry.file_name().to_string_lossy().into_owned());
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
        let mut threads = Vec::new();
        for id in ids {
            let store = self.thread(&id)?;
            let document = store.load()?;
            let updated = std::fs::metadata(store.root.join("conversation.json"))
                .and_then(|m| m.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |time| time.as_secs());
            threads.push(ThreadInfo {
                id,
                title: document.sidebar_title(),
                archived: document.archived,
                updated,
            });
        }
        threads.sort_by(|a, b| b.updated.cmp(&a.updated).then_with(|| a.id.cmp(&b.id)));
        Ok(threads)
    }
    /// Stable identity for cross-project choice evidence.
    pub fn identity(&self) -> String {
        self.root.to_string_lossy().into_owned()
    }
    /// Artifact path allocated by the host.
    pub fn candidate_path(&self, id: u64) -> PathBuf {
        self.root.join(id.to_string())
    }
    /// Load the full conversation or start a new one.
    pub fn load(&self) -> Result<Document, String> {
        if self.thread_id != "default" && !self.root.join("conversation.json").is_file() {
            return Err("Song conversation is missing".into());
        }
        let mut document: Document = read_or_default(&self.root.join("conversation.json"))?;
        document.thread_id = self.thread_id.clone();
        for message in &mut document.messages {
            for path in &mut message.attachments {
                *path = crate::conversation_attachments::resolve(&self.root, path);
            }
            for reference in &mut message.references {
                reference.path =
                    crate::conversation_attachments::resolve(&self.root, &reference.path);
            }
        }
        Ok(document)
    }
    /// Atomically save a conversation.
    pub fn save(&self, document: &Document) -> Result<(), String> {
        let mut document = document.clone();
        document.thread_id = self.thread_id.clone();
        for message in &mut document.messages {
            for path in &mut message.attachments {
                *path = crate::conversation_attachments::portable(&self.root, path);
            }
            for reference in &mut message.references {
                reference.path =
                    crate::conversation_attachments::portable(&self.root, &reference.path);
            }
        }
        write(&self.root.join("conversation.json"), &document)
    }
    /// Preserve explicitly attached files before passing them to the director.
    pub fn capture_attachments(&self, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        crate::conversation_attachments::capture(&self.root, paths)
    }
    /// Return only references owned by this workshop; restored metadata cannot grant file access.
    pub fn owned_attachments(&self, paths: &[PathBuf]) -> Vec<PathBuf> {
        paths
            .iter()
            .filter(|path| crate::conversation_attachments::owns(&self.root, path))
            .cloned()
            .collect()
    }
}

/// Read local cross-project preference evidence.
pub fn load_preferences() -> Result<Preferences, String> {
    read_or_default(&crate::config_dir().join("song-preferences.json"))
}

/// Update preferences under a process-safe lock to avoid losing another window's choice.
pub fn update_preferences(update: impl FnOnce(&mut Preferences)) -> Result<Preferences, String> {
    try_update_preferences(|preferences| {
        update(preferences);
        Ok(())
    })
}

/// Atomically correct or remove an existing preference without overwriting concurrent changes.
pub fn revise_preference(
    original: &Selection,
    correction: Option<(String, usize)>,
) -> Result<Preferences, String> {
    try_update_preferences(|preferences| preferences.revise_selection(original, correction))
}

fn try_update_preferences(
    update: impl FnOnce(&mut Preferences) -> Result<(), String>,
) -> Result<Preferences, String> {
    let root = crate::config_dir();
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(root.join("song-preferences.lock"))
        .map_err(|e| e.to_string())?;
    lock.lock().map_err(|e| e.to_string())?;
    let mut preferences = load_preferences()?;
    update(&mut preferences)?;
    write(&root.join("song-preferences.json"), &preferences)?;
    Ok(preferences)
}

fn read_or_default<T: serde::de::DeserializeOwned + Default>(path: &Path) -> Result<T, String> {
    match std::fs::metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(T::default()),
        Err(e) => Err(e.to_string()),
        Ok(meta) if meta.len() > 8 * 1024 * 1024 => Err("Workshop history is too large".into()),
        Ok(_) => serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string()),
    }
}

fn write(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > 8 * 1024 * 1024 {
        return Err("Workshop history is too large".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::settings::write_config_bytes(path, &bytes).map_err(|e| e.to_string())
}

fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    for entry in std::fs::read_dir(source).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let kind = entry.file_type().map_err(|e| e.to_string())?;
        if entry.file_name() == "workshop.lock" {
            continue;
        }
        if kind.is_symlink() {
            return Err("Workshop artifacts may not contain symbolic links".into());
        }
        let destination = target.join(entry.file_name());
        if kind.is_dir() {
            std::fs::create_dir_all(&destination).map_err(|e| e.to_string())?;
            copy_tree(&entry.path(), &destination)?;
        } else {
            std::fs::copy(entry.path(), destination).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round(ids: &[u64], ready: bool) -> Round {
        Round {
            after_message: 0,
            candidates: ids
                .iter()
                .map(|id| Candidate {
                    id: *id,
                    parent: None,
                    direction: Direction {
                        title: "Song".into(),
                        description: "Piano".into(),
                        instructions: "Compose".into(),
                        protected_tracks: Vec::new(),
                    },
                    ready,
                    error: None,
                    recoveries: 0,
                    tools: Default::default(),
                    feedback: None,
                })
                .collect(),
        }
    }

    #[test]
    fn continuation_uses_the_latest_single_finished_song_without_selection() {
        let document = Document {
            rounds: vec![round(&[1, 2], true), round(&[3], true)],
            ..Default::default()
        };
        assert_eq!(document.continuation_candidate().map(|c| c.id), Some(3));
    }

    #[test]
    fn continuation_keeps_the_last_song_after_an_unsuccessful_revision() {
        let document = Document {
            rounds: vec![round(&[1], true), round(&[2], false)],
            ..Default::default()
        };
        assert_eq!(document.continuation_candidate().map(|c| c.id), Some(1));
    }

    #[test]
    fn continuation_does_not_choose_between_alternatives_even_when_one_failed() {
        let mut comparison = round(&[2, 3], true);
        comparison.candidates[1].ready = false;
        let document = Document {
            rounds: vec![round(&[1], true), comparison],
            ..Default::default()
        };
        assert!(document.continuation_candidate().is_none());
    }

    #[test]
    fn explicit_selection_takes_precedence_over_an_implicit_baseline() {
        let document = Document {
            rounds: vec![round(&[1, 2], true), round(&[3], true)],
            selected: Some(2),
            ..Default::default()
        };
        assert_eq!(document.continuation_candidate().map(|c| c.id), Some(2));
    }

    #[test]
    fn unfinished_songs_cannot_be_used_as_a_continuation_baseline() {
        let document = Document {
            rounds: vec![round(&[1], false)],
            ..Default::default()
        };
        assert!(document.continuation_candidate().is_none());
        assert!(Document::default().continuation_candidate().is_none());
    }

    #[test]
    fn song_threads_preserve_legacy_history_isolate_artifacts_and_follow_save_as() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::new(directory.path().join("source"));
        let mut legacy = Document {
            draft: "Original draft".into(),
            ..Default::default()
        };
        legacy.rename("Original conversation").unwrap();
        store.save(&legacy).unwrap();
        std::fs::create_dir_all(store.candidate_path(1)).unwrap();
        std::fs::write(store.candidate_path(1).join("marker"), "original audio").unwrap();
        let other = store.create_thread().unwrap();
        let mut second = other.load().unwrap();
        second.draft = "Independent draft".into();
        second.rename("Second conversation").unwrap();
        second.archived = true;
        other.save(&second).unwrap();
        std::fs::create_dir_all(other.candidate_path(1)).unwrap();
        std::fs::write(other.candidate_path(1).join("marker"), "other audio").unwrap();
        other.set_active_thread().unwrap();
        assert_eq!(
            store.active_thread().unwrap().load().unwrap().draft,
            "Independent draft"
        );
        assert_eq!(store.load().unwrap().draft, "Original draft");
        assert_eq!(store.threads().unwrap().len(), 2);
        assert!(
            store
                .threads()
                .unwrap()
                .iter()
                .any(|t| t.title == "Second conversation" && t.archived)
        );
        for id in [
            "../escape",
            "song-../../escape",
            "C:\\elsewhere",
            "song-/escape",
        ] {
            assert!(store.thread(id).is_err());
        }
        assert!(store.thread("song-missing").unwrap().load().is_err());
        let destination = Store::new(directory.path().join("copy"));
        store.copy_to(&destination).unwrap();
        let restored = destination.active_thread().unwrap();
        assert_eq!(restored.load().unwrap().title, "Second conversation");
        assert_eq!(
            std::fs::read_to_string(restored.candidate_path(1).join("marker")).unwrap(),
            "other audio"
        );
        assert_eq!(
            std::fs::read_to_string(destination.candidate_path(1).join("marker")).unwrap(),
            "original audio"
        );
    }
    #[test]
    fn feedback_is_validated_replaced_and_restored_with_its_exact_candidate() {
        let folder = tempfile::tempdir().unwrap();
        let store = Store::new(folder.path().into());
        let candidate: Candidate = serde_json::from_value(serde_json::json!({
            "id": 4, "parent": null, "direction": {"title":"Song", "description":"Piano", "instructions":"Compose"},
            "ready": true, "error": null
        })).unwrap();
        assert!(candidate.feedback.is_none());
        let mut document = Document {
            rounds: vec![Round {
                after_message: 0,
                candidates: vec![candidate],
            }],
            ..Default::default()
        };
        assert!(
            document
                .record_feedback(
                    4,
                    Feedback {
                        rating: 0,
                        comment: String::new()
                    }
                )
                .is_err()
        );
        assert!(
            document
                .record_feedback(
                    4,
                    Feedback {
                        rating: 6,
                        comment: String::new()
                    }
                )
                .is_err()
        );
        assert!(
            document
                .record_feedback(
                    4,
                    Feedback {
                        rating: 3,
                        comment: "感".repeat(2001)
                    }
                )
                .is_err()
        );
        assert!(
            document
                .record_feedback(
                    9,
                    Feedback {
                        rating: 3,
                        comment: String::new()
                    }
                )
                .is_err()
        );
        document
            .record_feedback(
                4,
                Feedback {
                    rating: 2,
                    comment: "Too busy".into(),
                },
            )
            .unwrap();
        let revised = Feedback {
            rating: 5,
            comment: "柔らかな音色が好き\n後半は静かに".into(),
        };
        document.record_feedback(4, revised.clone()).unwrap();
        store.save(&document).unwrap();
        assert_eq!(
            store.load().unwrap().rounds[0].candidates[0].feedback,
            Some(revised)
        );
        document.rounds[0].candidates[0].ready = false;
        assert!(
            document
                .record_feedback(
                    4,
                    Feedback {
                        rating: 3,
                        comment: String::new()
                    }
                )
                .is_err()
        );
    }
    #[test]
    fn interrupted_calls_remain_distinct_from_tool_failures_after_reload() {
        let mut usage = ToolUsage {
            total: 5,
            succeeded: 2,
            failed: 1,
            interrupted: 0,
        };
        assert_eq!(usage.running(), 2);
        usage.interrupt();
        usage.interrupt();
        let saved = serde_json::to_string(&usage).unwrap();
        let restored: ToolUsage = serde_json::from_str(&saved).unwrap();
        assert_eq!(
            restored,
            ToolUsage {
                total: 5,
                succeeded: 2,
                failed: 1,
                interrupted: 2
            }
        );
        assert_eq!(restored.running(), 0);
    }

    #[test]
    fn older_candidates_open_without_tool_usage() {
        let candidate: Candidate = serde_json::from_value(serde_json::json!({
            "id": 1, "parent": null, "direction": {"title":"Song", "description":"Piano", "instructions":"Compose"},
            "ready": false, "error": null
        })).unwrap();
        assert!(candidate.tools.is_empty());
    }
    #[test]
    fn attached_files_follow_the_workshop_without_trusting_external_history_paths() {
        let folder = tempfile::tempdir().unwrap();
        let source = folder.path().join("lyrics.txt");
        std::fs::write(&source, "夜の雨").unwrap();
        let store = Store::new(folder.path().join("first"));
        let attachments = store
            .capture_attachments(std::slice::from_ref(&source))
            .unwrap();
        let mut document = Document::default();
        document.messages.push(Message {
            choices: None,
            user: true,
            text: "Use these lyrics".into(),
            references: vec![Reference {
                path: attachments[0].clone(),
                text: "夜の雨".into(),
            }],
            attachments,
        });
        store.save(&document).unwrap();
        let destination = Store::new(folder.path().join("second"));
        store.copy_to(&destination).unwrap();
        let restored = destination.load().unwrap();
        let files = destination.owned_attachments(&restored.messages[0].attachments);
        assert_eq!(files.len(), 1);
        assert_eq!(restored.messages[0].references[0].path, files[0]);
        assert_eq!(std::fs::read_to_string(&files[0]).unwrap(), "夜の雨");
        assert!(destination.owned_attachments(&[source]).is_empty());
        let legacy: Message =
            serde_json::from_str(r#"{"user":true,"text":"Older message"}"#).unwrap();
        assert!(legacy.attachments.is_empty());
    }
    fn selection(round: u64, candidate: u64) -> Selection {
        Selection {
            conversation: "test".into(),
            round,
            candidate,
            context: "A calm theme".into(),
            alternatives: vec![Direction {
                title: "Piano".into(),
                description: "Sparse".into(),
                instructions: "Private production instructions".into(),
                protected_tracks: vec![],
            }],
            chosen: 0,
        }
    }
    #[test]
    fn corrections_change_retrieved_evidence_and_preserve_provenance() {
        let mut original = selection(1, 1);
        let mut alternative = original.alternatives[0].clone();
        alternative.title = "Strings".into();
        original.alternatives.push(alternative);
        let mut preferences = Preferences::default();
        preferences.record(original.clone());
        preferences.record(selection(2, 2));
        preferences
            .revise_selection(&original, Some(("A warm theme\nWith strings".into(), 1)))
            .unwrap();
        let corrected = &preferences.selections[1];
        assert_eq!(corrected.conversation, original.conversation);
        assert_eq!(corrected.candidate, original.candidate);
        assert_eq!(corrected.round, original.round);
        assert_eq!(corrected.alternatives, original.alternatives);
        let evidence: serde_json::Value = serde_json::from_str(&preferences.context()).unwrap();
        assert_eq!(evidence[0]["chosen"], 1);
        assert_eq!(evidence[0]["context"], "A warm theme\nWith strings");
        assert_eq!(preferences.selections[0], selection(2, 2));
    }

    #[test]
    fn correcting_older_history_brings_it_into_the_next_retrieval() {
        let original = selection(1, 1);
        let mut preferences = Preferences::default();
        for id in 1..=12 {
            preferences.record(selection(id, id));
        }
        preferences
            .revise_selection(&original, Some(("Corrected older request".into(), 0)))
            .unwrap();
        let evidence: serde_json::Value = serde_json::from_str(&preferences.context()).unwrap();
        assert_eq!(evidence[0]["context"], "Corrected older request");
        assert_eq!(preferences.selections.len(), 12);
    }

    #[test]
    fn stale_corrections_cannot_restore_reset_or_replaced_choices() {
        let original = selection(1, 1);
        let mut preferences = Preferences::default();
        preferences.record(original.clone());
        preferences.record(selection(1, 2));
        assert!(preferences.revise_selection(&original, None).is_err());
        assert_eq!(preferences.selections[0].candidate, 2);
        preferences.selections.clear();
        assert!(
            preferences
                .revise_selection(&original, Some(("Stale".into(), 0)))
                .is_err()
        );
        assert!(preferences.selections.is_empty());
    }

    #[test]
    fn invalid_correction_preserves_history_and_disabled_history_remains_editable() {
        let original = selection(1, 1);
        let mut preferences = Preferences::default();
        preferences.record(original.clone());
        for correction in [(" ".into(), 0), ("Valid".into(), 99)] {
            assert!(
                preferences
                    .revise_selection(&original, Some(correction))
                    .is_err()
            );
            assert_eq!(preferences.selections[0], original);
        }
        preferences.enabled = false;
        preferences.revise_selection(&original, None).unwrap();
        assert!(preferences.selections.is_empty());
        assert!(!preferences.enabled);
    }
    #[test]
    fn reselecting_replaces_one_vote_but_same_titles_in_new_round_count() {
        let mut preferences = Preferences::default();
        preferences.record(selection(1, 1));
        preferences.record(selection(1, 2));
        preferences.record(selection(4, 4));
        assert_eq!(
            preferences
                .selections
                .iter()
                .map(|s| s.candidate)
                .collect::<Vec<_>>(),
            [2, 4]
        );
        assert!(
            !preferences
                .context()
                .contains("Private production instructions")
        );
    }
    #[test]
    fn save_as_copies_relative_artifacts_and_leaves_source_usable() {
        let source = tempfile::tempdir().unwrap();
        let destination = tempfile::tempdir().unwrap();
        let original = Store::new(source.path().into());
        original
            .save(&Document {
                selected: Some(3),
                ..Default::default()
            })
            .unwrap();
        std::fs::create_dir_all(original.candidate_path(3)).unwrap();
        std::fs::write(original.candidate_path(3).join("asset.wav"), b"retained").unwrap();
        let target = Store::new(destination.path().into());
        original.copy_to(&target).unwrap();
        assert_eq!(target.load().unwrap().selected, Some(3));
        assert_eq!(
            std::fs::read(target.candidate_path(3).join("asset.wav")).unwrap(),
            b"retained"
        );
        assert!(original.candidate_path(3).join("asset.wav").exists());
    }
    #[test]
    fn unreadable_history_is_not_replaced_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().into());
        std::fs::create_dir_all(&store.root).unwrap();
        let path = store.root.join("conversation.json");
        std::fs::write(&path, b"broken").unwrap();
        assert!(store.load().is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"broken");
    }
    #[test]
    fn transcript_and_parent_survive_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::new(dir.path().into());
        let mut document = Document::default();
        document.messages.push(Message {
            choices: None,
            references: vec![],
            attachments: Vec::new(),
            user: true,
            text: "歌詞\n二行目".into(),
        });
        document.selected = Some(7);
        store.save(&document).unwrap();
        let restored = store.load().unwrap();
        assert_eq!(restored.messages[0].text, "歌詞\n二行目");
        assert_eq!(restored.selected, Some(7));
    }
    #[test]
    fn disabled_memory_does_not_record_or_disclose_choices() {
        let mut preferences = Preferences {
            enabled: false,
            ..Default::default()
        };
        preferences.record(selection(1, 1));
        assert!(preferences.selections.is_empty());
        assert!(preferences.context().is_empty());
    }
}
