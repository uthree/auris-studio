//! Project-local agent conversations, independent of the score and its undo history.
//!
//! Call storage operations off the UI thread. A stable store lock serializes metadata changes,
//! while transcript revisions reject writes from stale windows. Model memory is owned by the
//! agent runtime and lives beside, rather than inside, the readable transcript.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const READ_LIMIT: u64 = 64 * 1024 * 1024;
static NEXT_ID: AtomicU64 = AtomicU64::new(0);
const RECOVERY_ID: &str = ".auris-agent-recovery.json";

/// Persistent identity and list information for a conversation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationInfo {
    /// Opaque directory name; never a user supplied path.
    pub id: String,
    /// User-visible name, initially derived from the first request.
    pub title: String,
    /// Creation time as milliseconds since the Unix epoch.
    pub created_at: u64,
    /// Last transcript change as milliseconds since the Unix epoch.
    pub updated_at: u64,
    /// Preview of the most recent user or assistant message.
    pub preview: String,
    /// Whether this conversation is hidden from the ordinary list.
    pub archived: bool,
    /// A tombstone preventing a late writer from resurrecting deleted history.
    #[serde(default)]
    pub deleted: bool,
}

/// One stable message in the readable transcript.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConversationMessage {
    /// Stable identity within this conversation, including streamed updates.
    pub id: u64,
    /// Model reported for this message, if known.
    #[serde(default)]
    pub model: String,
    /// The contents shown to the reader.
    pub content: ConversationContent,
}

/// Provider-independent transcript contents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ConversationContent {
    /// A request from the user.
    User {
        /// Request text.
        text: String,
        /// Display-only references sent with this message, never restored as attachment access.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        attachments: Vec<PathBuf>,
    },
    /// Visible assistant output.
    Assistant {
        /// Answer text.
        text: String,
    },
    /// Collapsible reasoning received from the provider.
    Reasoning {
        /// Reasoning text.
        text: String,
    },
    /// Runtime information or an interruption notice.
    Status {
        /// Status text.
        text: String,
    },
    /// A summary retained from a legacy conversation.
    Context {
        /// Historical reference context.
        text: String,
    },
    /// A tool invocation and its observed result.
    Tool {
        /// Tool wire name.
        name: String,
        /// JSON arguments.
        args: String,
        /// Whether the tool succeeded.
        ok: bool,
        /// Short result, or empty while running.
        line: String,
        /// Complete textual result.
        detail: String,
    },
    /// A question the agent asked with fixed answers, and what was picked.
    Choices {
        /// The question and its options.
        card: crate::choices::ChoiceCard,
        /// Indices picked, in the order ticked; the ones sent once answered.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        picked: Vec<usize>,
        /// Whether the pick went to the agent.
        #[serde(default)]
        answered: bool,
    },
    /// A provider or host failure.
    Error {
        /// Error text.
        text: String,
    },
}

/// One user-authored message waiting to be submitted to an agent.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueuedMessage {
    /// User-authored text waiting for its own turn.
    pub text: String,
    /// Explicit reference files; restoring paths does not grant permission to send them.
    #[serde(default)]
    pub attachments: Vec<PathBuf>,
    /// Selection at submission time, using stable document IDs.
    #[serde(default)]
    pub selection_context: String,
}

/// Readable history and composer state; model context is stored separately.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ConversationTranscript {
    /// Compare-and-swap revision for this transcript.
    pub revision: u64,
    /// Messages in display order.
    pub messages: Vec<ConversationMessage>,
    /// Unsent text belonging to this conversation.
    pub draft: String,
    /// Reference files belonging to the unsent draft.
    #[serde(default)]
    pub attachments: Vec<PathBuf>,
    /// Vertical scroll offset in logical pixels.
    #[serde(default)]
    pub scroll_y: f32,
    /// A turn was in progress when the latest snapshot was written.
    #[serde(default)]
    pub interrupted: bool,
    /// Unsent messages, restored paused until the user resumes them.
    #[serde(default)]
    pub queued: Vec<QueuedMessage>,
}

/// One loaded conversation.
#[derive(Clone, Debug, PartialEq)]
pub struct Conversation {
    /// Metadata used in the list.
    pub info: ConversationInfo,
    /// The readable history and draft.
    pub transcript: ConversationTranscript,
}

/// File storage for one project's agent conversations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationStore {
    root: PathBuf,
}

impl ConversationStore {
    /// Resolve a private persistent store associated with an unsaved document's workspace.
    /// The small identity file follows autosave recovery; conversation contents stay outside
    /// the temporary workspace so a normal workspace cleanup cannot interrupt a final save.
    pub fn for_unsaved_document(workspace: &Path) -> Result<Self, String> {
        let marker = workspace.join(RECOVERY_ID);
        let id: String = if marker.exists() {
            read_json(&marker)?
        } else {
            let id = new_id();
            write_json(&marker, &id)?;
            id
        };
        if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err("Invalid conversation recovery identity".into());
        }
        Ok(Self::at(
            crate::settings::config_dir()
                .join("agent-recovery")
                .join(id),
        ))
    }

    /// Carry an unsaved conversation's identity into a promoted or recovered workspace.
    pub fn copy_recovery_identity(from: &Path, to: &Path) -> Result<(), String> {
        let source = from.join(RECOVERY_ID);
        if source.exists() {
            let id: String = read_json(&source)?;
            write_json(&to.join(RECOVERY_ID), &id)?;
        }
        Ok(())
    }

    /// Store conversations inside a movable project folder.
    pub fn for_project(folder: &Path) -> Self {
        Self {
            root: folder.join(".auris/agent/conversations"),
        }
    }

    /// Open a private recovery store, for a document which has not been saved yet.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    /// Absolute storage location, also useful for recovery discovery.
    pub fn root(&self) -> &Path {
        &self.root
    }

    fn lock(&self) -> Result<File, String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join("store.lock"))
            .map_err(|e| e.to_string())?;
        file.lock().map_err(|e| e.to_string())?;
        Ok(file)
    }

    fn folder(&self, id: &str) -> Result<PathBuf, String> {
        if id.is_empty() || !id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
            return Err("Invalid conversation identity".into());
        }
        Ok(self.root.join(id))
    }

    /// Path to the runtime's bounded model memory for a validated conversation ID.
    pub fn memory_path(&self, id: &str) -> Result<PathBuf, String> {
        Ok(self.folder(id)?.join("memory.json"))
    }

    /// Archive files explicitly attached by the user before sending a message.
    pub fn capture_attachments(&self, id: &str, paths: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
        let _lock = self.lock()?;
        self.info(id)?;
        crate::conversation_attachments::capture(&self.folder(id)?, paths)
    }

    fn info(&self, id: &str) -> Result<ConversationInfo, String> {
        let info: ConversationInfo = read_json(&self.folder(id)?.join("meta.json"))?;
        if info.id != id || info.deleted {
            return Err("Conversation no longer exists".into());
        }
        Ok(info)
    }

    /// List metadata only, newest first, without opening transcripts or starting providers.
    pub fn list(&self) -> Result<Vec<ConversationInfo>, String> {
        let _lock = self.lock()?;
        self.list_unlocked()
    }

    fn list_unlocked(&self) -> Result<Vec<ConversationInfo>, String> {
        let mut result = Vec::new();
        for entry in std::fs::read_dir(&self.root).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if !entry.file_type().map_err(|e| e.to_string())?.is_dir() {
                continue;
            }
            let id = entry.file_name().to_string_lossy().into_owned();
            let path = self.folder(&id)?.join("meta.json");
            if !path.exists() {
                continue;
            }
            let info: ConversationInfo = read_json(&path)?;
            if info.id != id {
                return Err("Conversation metadata has a different identity".into());
            }
            if !info.deleted {
                result.push(info);
            }
        }
        result.sort_by(|a, b| {
            b.updated_at
                .cmp(&a.updated_at)
                .then_with(|| a.id.cmp(&b.id))
        });
        Ok(result)
    }

    /// Create a distinct conversation without replacing any existing history.
    pub fn create(&self) -> Result<Conversation, String> {
        let _lock = self.lock()?;
        self.create_unlocked()
    }

    fn create_unlocked(&self) -> Result<Conversation, String> {
        let now = now_millis();
        let id = new_id();
        let folder = self.folder(&id)?;
        std::fs::create_dir(&folder).map_err(|e| e.to_string())?;
        let info = ConversationInfo {
            id,
            title: String::new(),
            created_at: now,
            updated_at: now,
            preview: String::new(),
            archived: false,
            deleted: false,
        };
        let transcript = ConversationTranscript::default();
        write_json(&folder.join("transcript.json"), &transcript)?;
        write_json(&folder.join("meta.json"), &info)?;
        Ok(Conversation { info, transcript })
    }

    /// Read one transcript independently of the model's compressed context.
    pub fn load(&self, id: &str) -> Result<Conversation, String> {
        let _lock = self.lock()?;
        let info = self.info(id)?;
        let folder = self.folder(id)?;
        let mut transcript = read_json(&folder.join("transcript.json"))?;
        map_attachments(&mut transcript, |path| {
            crate::conversation_attachments::resolve(&folder, path)
        });
        Ok(Conversation { info, transcript })
    }

    /// Atomically replace a transcript if its revision still matches the observed snapshot.
    /// Metadata edits from other windows are preserved.
    pub fn save(
        &self,
        id: &str,
        transcript: &ConversationTranscript,
    ) -> Result<Conversation, String> {
        let _lock = self.lock()?;
        let mut info = self.info(id)?;
        let folder = self.folder(id)?;
        let mut previous: ConversationTranscript = read_json(&folder.join("transcript.json"))?;
        map_attachments(&mut previous, |path| {
            crate::conversation_attachments::resolve(&folder, path)
        });
        if previous.revision != transcript.revision {
            return Err(
                "Conversation changed in another window. Reopen it before continuing.".into(),
            );
        }
        let mut transcript = transcript.clone();
        transcript.revision = transcript
            .revision
            .checked_add(1)
            .ok_or("Conversation revision exhausted")?;
        if info.title.is_empty()
            && let Some(text) = transcript.messages.iter().find_map(|m| match &m.content {
                ConversationContent::User { text, .. } => Some(text),
                _ => None,
            })
        {
            info.title = preview(text, 60);
        }
        info.preview = transcript
            .messages
            .iter()
            .rev()
            .find_map(|m| match &m.content {
                ConversationContent::User { text, .. }
                | ConversationContent::Assistant { text } => Some(preview(text, 100)),
                _ => None,
            })
            .unwrap_or_default();
        if previous.messages != transcript.messages {
            info.updated_at = now_millis();
        }
        let mut stored = transcript.clone();
        map_attachments(&mut stored, |path| {
            crate::conversation_attachments::portable(&folder, path)
        });
        write_json(&folder.join("transcript.json"), &stored)?;
        write_json(&folder.join("meta.json"), &info)?;
        Ok(Conversation { info, transcript })
    }

    /// Rename a conversation without changing its identity or model memory.
    pub fn rename(&self, id: &str, title: &str) -> Result<(), String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("Enter a conversation name".into());
        }
        let _lock = self.lock()?;
        let mut info = self.info(id)?;
        info.title = preview(title, 120);
        write_json(&self.folder(id)?.join("meta.json"), &info)
    }

    /// Hide or restore a conversation in the ordinary list.
    pub fn archive(&self, id: &str, archived: bool) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut info = self.info(id)?;
        info.archived = archived;
        write_json(&self.folder(id)?.join("meta.json"), &info)
    }

    /// Tombstone a conversation and remove its readable transcript.
    /// The caller must invalidate the runtime's memory generation before calling this method.
    pub fn delete(&self, id: &str) -> Result<(), String> {
        let _lock = self.lock()?;
        let mut info = self.info(id)?;
        info.deleted = true;
        info.title.clear();
        info.preview.clear();
        let folder = self.folder(id)?;
        write_json(&folder.join("meta.json"), &info)?;
        std::fs::remove_file(folder.join("transcript.json")).map_err(|e| e.to_string())?;
        let attachments = folder.join("attachments");
        if attachments.exists() {
            std::fs::remove_dir_all(attachments).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Import the old single-conversation memory exactly once, preserving the source file.
    /// Legacy files contain completed turns and a summary, but no tool execution details.
    pub fn migrate_legacy(&self, path: &Path) -> Result<(), String> {
        let _lock = self.lock()?;
        let marker = self.root.join("legacy-imported.json");
        if marker.exists() || !path.exists() {
            return Ok(());
        }
        let memory: serde_json::Value = read_json(path)?;
        let turns = memory["turns"]
            .as_array()
            .ok_or("Cannot read legacy conversation turns")?;
        if turns
            .iter()
            .any(|turn| !turn["user"].is_string() || !turn["answer"].is_string())
            || memory
                .get("summary")
                .is_some_and(|summary| !summary.is_string())
        {
            return Err("Cannot read legacy conversation text".into());
        }
        let id = "legacy";
        let folder = self.folder(id)?;
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        // A deterministic identity makes retrying after a crash idempotent.
        if !folder.join("meta.json").exists() {
            let mut transcript = ConversationTranscript::default();
            if let Some(text) = memory["summary"].as_str().filter(|s| !s.is_empty()) {
                transcript.messages.push(ConversationMessage {
                    id: 0,
                    model: String::new(),
                    content: ConversationContent::Context { text: text.into() },
                });
            }
            for turn in memory["turns"].as_array().into_iter().flatten() {
                for (key, user) in [("user", true), ("answer", false)] {
                    if let Some(text) = turn[key].as_str() {
                        transcript.messages.push(ConversationMessage {
                            id: transcript.messages.len() as u64,
                            model: String::new(),
                            content: if user {
                                ConversationContent::User {
                                    text: text.into(),
                                    attachments: Vec::new(),
                                }
                            } else {
                                ConversationContent::Assistant { text: text.into() }
                            },
                        });
                    }
                }
            }
            let title = transcript
                .messages
                .iter()
                .find_map(|m| match &m.content {
                    ConversationContent::User { text, .. } => Some(preview(text, 60)),
                    _ => None,
                })
                .unwrap_or_default();
            let now = now_millis();
            let info = ConversationInfo {
                id: id.into(),
                title,
                created_at: now,
                updated_at: now,
                preview: turns
                    .last()
                    .and_then(|turn| turn["answer"].as_str())
                    .map(|text| preview(text, 100))
                    .unwrap_or_default(),
                archived: false,
                deleted: false,
            };
            write_json(&folder.join("memory.json"), &memory)?;
            write_json(&folder.join("transcript.json"), &transcript)?;
            write_json(&folder.join("meta.json"), &info)?;
        }
        write_json(&marker, &true)
    }

    /// Copy a recovery or Save As store into a new project without moving its source.
    pub fn copy_to(&self, destination: &Self) -> Result<(), String> {
        if self == destination {
            return Ok(());
        }
        // Resolve both stores before locking. Save As can name the current folder through a
        // relative path, different case or a symlink; locking both spellings would wait on our
        // own first lock. Canonical roots also give opposite-direction copies the same order,
        // even when either caller reached one of the stores through an alias.
        let source = self.canonical_copy_store()?;
        let destination = destination.canonical_copy_store()?;
        if same_file::is_same_file(&source.root, &destination.root).map_err(|e| e.to_string())? {
            return Ok(());
        }
        let (first, second) = if source.copy_order_key() < destination.copy_order_key() {
            (&source, &destination)
        } else {
            (&destination, &source)
        };
        let _first_lock = first.lock()?;
        let _second_lock = second.lock()?;
        for info in source.list_unlocked()? {
            let source = source.folder(&info.id)?;
            let target = destination.folder(&info.id)?;
            if target.join("meta.json").exists() {
                let existing: ConversationInfo = read_json(&target.join("meta.json"))?;
                let same_transcript = std::fs::read(source.join("transcript.json"))
                    .map_err(|e| e.to_string())?
                    == std::fs::read(target.join("transcript.json")).map_err(|e| e.to_string())?;
                let same_production = ["memory.production.json"].into_iter().all(|name| {
                    let read = |folder: &Path| match std::fs::read(folder.join(name)) {
                        Ok(bytes) => Ok(Some(bytes)),
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                        Err(error) => Err(error),
                    };
                    matches!((read(&source), read(&target)), (Ok(a), Ok(b)) if a == b)
                });
                if existing == info && same_transcript && same_production {
                    crate::conversation_attachments::copy_to(&source, &target)?;
                    continue;
                }
                return Err(
                    "Destination already contains a different version of this conversation".into(),
                );
            }
            std::fs::create_dir_all(&target).map_err(|e| e.to_string())?;
            for name in ["transcript.json", "memory.json", "memory.production.json"] {
                let path = source.join(name);
                if path.exists() {
                    std::fs::copy(path, target.join(name)).map_err(|e| e.to_string())?;
                }
            }
            write_json(&target.join("meta.json"), &info)?;
            crate::conversation_attachments::copy_to(&source, &target)?;
        }
        Ok(())
    }

    fn canonical_copy_store(&self) -> Result<Self, String> {
        std::fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let root = self.root.canonicalize().map_err(|e| e.to_string())?;
        Ok(Self::at(root))
    }

    fn copy_order_key(&self) -> (PathBuf, PathBuf) {
        let folded = if cfg!(windows) {
            self.root.as_os_str().to_ascii_lowercase().into()
        } else {
            self.root.clone()
        };
        // The full path breaks ties for distinct case-sensitive Windows directories.
        (folded, self.root.clone())
    }
}

fn map_attachments(transcript: &mut ConversationTranscript, map: impl Fn(&Path) -> PathBuf) {
    for message in &mut transcript.messages {
        if let ConversationContent::User { attachments, .. } = &mut message.content {
            for path in attachments {
                *path = map(path);
            }
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let nonce = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    format!(
        "{:x}-{:x}-{:x}",
        now_millis(),
        nonce,
        NEXT_ID.fetch_add(1, Ordering::Relaxed)
    )
}

fn preview(text: &str, limit: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    if file.metadata().map_err(|e| e.to_string())?.len() > READ_LIMIT {
        return Err("Conversation file is too large".into());
    }
    let mut bytes = Vec::new();
    file.take(READ_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > READ_LIMIT {
        return Err("Conversation file is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Cannot read conversation: {e}"))
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() as u64 > READ_LIMIT {
        return Err("Conversation file is too large".into());
    }
    crate::settings::write_file_atomic(path, &bytes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copying_store_aliases_and_opposite_directions_does_not_block() {
        const PROBE: &str = "AURIS_CONVERSATION_COPY_TEST";
        if let Ok(mode) = std::env::var(PROBE) {
            let root = std::env::current_dir().unwrap();
            let source = ConversationStore::at(PathBuf::from("source"));
            let conversation = source.create().unwrap();
            let attachment = root.join("reference.txt");
            std::fs::write(&attachment, "Retain the complete attachment").unwrap();
            let archived = source
                .capture_attachments(&conversation.info.id, &[attachment])
                .unwrap();
            let bytes = std::fs::read(&archived[0]).unwrap();
            let destination = ConversationStore::at(root.join(match mode.as_str() {
                "relative" => "source",
                "case" => "SOURCE",
                "symlink" => "alias",
                "opposite" => "target",
                _ => panic!("unknown copy probe"),
            }));
            if mode == "opposite" {
                destination.create().unwrap();
                let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
                let handles: Vec<_> = [
                    (source.clone(), destination.clone()),
                    (
                        ConversationStore::at(PathBuf::from("target")),
                        ConversationStore::at(root.join("source")),
                    ),
                ]
                .into_iter()
                .map(|(from, to)| {
                    let barrier = barrier.clone();
                    std::thread::spawn(move || {
                        barrier.wait();
                        for _ in 0..16 {
                            from.copy_to(&to).unwrap();
                        }
                    })
                })
                .collect();
                for handle in handles {
                    handle.join().unwrap();
                }
                assert_eq!(source.list().unwrap(), destination.list().unwrap());
                assert_eq!(source.list().unwrap().len(), 2);
            } else {
                source.copy_to(&destination).unwrap();
                assert_eq!(
                    destination.load(&conversation.info.id).unwrap(),
                    conversation
                );
            }
            assert_eq!(std::fs::read(&archived[0]).unwrap(), bytes);
            return;
        }

        for mode in ["relative", "case", "symlink", "opposite"] {
            let directory = tempfile::tempdir().unwrap();
            std::fs::create_dir(directory.path().join("source")).unwrap();
            if mode == "case"
                && !same_file::is_same_file(
                    directory.path().join("source"),
                    directory.path().join("SOURCE"),
                )
                .unwrap_or(false)
            {
                continue;
            }
            if mode == "symlink" {
                #[cfg(unix)]
                std::os::unix::fs::symlink("source", directory.path().join("alias")).unwrap();
                #[cfg(not(unix))]
                continue;
            }
            // A broken alias check must fail this test instead of hanging the suite forever.
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "conversations::tests::copying_store_aliases_and_opposite_directions_does_not_block",
                ])
                .env(PROBE, mode)
                .current_dir(directory.path())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
            loop {
                if child.try_wait().unwrap().is_some() {
                    let output = child.wait_with_output().unwrap();
                    assert!(
                        output.status.success(),
                        "{mode}: {}\n{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    break;
                }
                if std::time::Instant::now() >= deadline {
                    child.kill().unwrap();
                    child.wait().unwrap();
                    panic!("{mode} conversation copy blocked");
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[test]
    fn sent_attachments_survive_original_deletion_and_save_as() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("reference.txt");
        std::fs::write(&source, "Original reference").unwrap();
        let store = ConversationStore::for_project(&dir.path().join("first"));
        let mut conversation = store.create().unwrap();
        let attachments = store
            .capture_attachments(&conversation.info.id, std::slice::from_ref(&source))
            .unwrap();
        conversation.transcript.messages.push(ConversationMessage {
            id: 0,
            model: String::new(),
            content: ConversationContent::User {
                text: "Use this reference".into(),
                attachments,
            },
        });
        store
            .save(&conversation.info.id, &conversation.transcript)
            .unwrap();
        std::fs::remove_file(source).unwrap();
        let destination = ConversationStore::for_project(&dir.path().join("second"));
        store.copy_to(&destination).unwrap();
        let loaded = destination.load(&conversation.info.id).unwrap();
        let ConversationContent::User { attachments, .. } = &loaded.transcript.messages[0].content
        else {
            panic!("user message")
        };
        assert!(attachments[0].starts_with(destination.root()));
        assert_eq!(
            std::fs::read_to_string(&attachments[0]).unwrap(),
            "Original reference"
        );
        let stored = std::fs::read_to_string(
            destination
                .folder(&conversation.info.id)
                .unwrap()
                .join("transcript.json"),
        )
        .unwrap();
        assert!(!stored.contains(&dir.path().to_string_lossy().replace('\\', "\\\\")));
    }

    #[test]
    fn transcripts_between_the_preference_ceiling_and_the_read_limit_still_save() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_project(dir.path());
        let conversation = store.create().unwrap();
        let mut transcript = conversation.transcript;
        transcript.messages.push(ConversationMessage {
            id: 0,
            model: String::new(),
            content: ConversationContent::Assistant {
                text: "音".repeat(3 * 1024 * 1024),
            },
        });
        let saved = store.save(&conversation.info.id, &transcript).unwrap();
        assert_eq!(
            store.load(&conversation.info.id).unwrap().transcript,
            saved.transcript
        );
    }

    #[test]
    fn queued_messages_round_trip_without_becoming_sent_history() {
        let old: ConversationTranscript =
            serde_json::from_str(r#"{"revision":0,"messages":[],"draft":"old draft"}"#).unwrap();
        assert!(old.queued.is_empty());
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_project(dir.path());
        let mut conversation = store.create().unwrap();
        conversation.transcript.queued.push(QueuedMessage {
            text: "Keep the melody".into(),
            attachments: vec![PathBuf::from("reference.png")],
            selection_context: "{\"selected_tracks\":[2]}".into(),
        });
        store
            .save(&conversation.info.id, &conversation.transcript)
            .unwrap();
        let restored = store.load(&conversation.info.id).unwrap();
        assert_eq!(restored.transcript.queued, conversation.transcript.queued);
        assert!(restored.transcript.messages.is_empty());
    }

    #[test]
    fn user_messages_retain_attachment_metadata_and_read_text_only_history() {
        let old: ConversationContent =
            serde_json::from_str(r#"{"kind":"user","text":"Listen to this"}"#).unwrap();
        assert_eq!(
            old,
            ConversationContent::User {
                text: "Listen to this".into(),
                attachments: Vec::new()
            }
        );
        let directory = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_project(directory.path());
        let conversation = store.create().unwrap();
        let mut transcript = conversation.transcript;
        let content = ConversationContent::User {
            text: "Listen to these references".into(),
            attachments: vec![
                PathBuf::from("References/歌.wav"),
                PathBuf::from("References/score.png"),
            ],
        };
        transcript.messages.push(ConversationMessage {
            id: 0,
            model: String::new(),
            content: content.clone(),
        });
        store.save(&conversation.info.id, &transcript).unwrap();
        assert_eq!(
            store
                .load(&conversation.info.id)
                .unwrap()
                .transcript
                .messages[0]
                .content,
            content
        );
    }

    #[test]
    fn conversations_keep_independent_history_and_reject_stale_writers() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_project(dir.path());
        let first = store.create().unwrap();
        let second = store.create().unwrap();
        let mut transcript = first.transcript.clone();
        transcript.messages.push(ConversationMessage {
            id: 7,
            model: "test-model".into(),
            content: ConversationContent::User {
                text: "Keep the bass".into(),
                attachments: Vec::new(),
            },
        });
        store.rename(&first.info.id, "Bass work").unwrap();
        let saved = store.save(&first.info.id, &transcript).unwrap();
        assert_eq!(saved.info.title, "Bass work");
        assert_eq!(saved.transcript.messages[0].id, 7);
        assert!(
            store
                .load(&second.info.id)
                .unwrap()
                .transcript
                .messages
                .is_empty()
        );
        assert!(
            store
                .save(&first.info.id, &transcript)
                .unwrap_err()
                .contains("another window")
        );
        store.archive(&first.info.id, true).unwrap();
        assert!(store.load(&first.info.id).unwrap().info.archived);
        store.delete(&first.info.id).unwrap();
        assert!(store.save(&first.info.id, &saved.transcript).is_err());
        assert_eq!(store.list().unwrap().len(), 1);
        assert!(store.load("../outside").is_err());
    }

    #[test]
    fn legacy_import_is_idempotent_and_survives_deletion_and_project_copy() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_project(dir.path());
        let legacy = dir.path().join(".auris-conversation.json");
        std::fs::write(
            &legacy,
            br#"{"summary":"Earlier context","turns":[{"user":"Bass","answer":"Added"}]}"#,
        )
        .unwrap();
        store.migrate_legacy(&legacy).unwrap();
        store.migrate_legacy(&legacy).unwrap();
        let loaded = store.load("legacy").unwrap();
        assert_eq!(loaded.transcript.messages.len(), 3);
        assert_eq!(loaded.info.title, "Bass");
        let dest_dir = tempfile::tempdir().unwrap();
        let dest = ConversationStore::for_project(dest_dir.path());
        store.copy_to(&dest).unwrap();
        assert_eq!(dest.load("legacy").unwrap(), loaded);
        store.delete("legacy").unwrap();
        store.migrate_legacy(&legacy).unwrap();
        assert!(store.list().unwrap().is_empty());
        assert!(legacy.exists());
    }

    #[test]
    fn project_copy_retains_production_records_and_rejects_conflicting_records() {
        let source_dir = tempfile::tempdir().unwrap();
        let target_dir = tempfile::tempdir().unwrap();
        let source = ConversationStore::for_project(source_dir.path());
        let target = ConversationStore::for_project(target_dir.path());
        let conversation = source.create().unwrap();
        let path = source
            .memory_path(&conversation.info.id)
            .unwrap()
            .with_extension("production.json");
        std::fs::write(&path, b"{\"entries\":[]}").unwrap();
        source.copy_to(&target).unwrap();
        let copied = target
            .memory_path(&conversation.info.id)
            .unwrap()
            .with_extension("production.json");
        assert_eq!(
            std::fs::read(&path).unwrap(),
            std::fs::read(copied).unwrap()
        );
        source.copy_to(&target).unwrap();
        std::fs::write(&path, b"{}").unwrap();
        assert!(source.copy_to(&target).is_err());
    }

    #[test]
    fn recovery_identity_follows_the_workspace_without_copying_history() {
        let original = tempfile::tempdir().unwrap();
        let promoted = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_unsaved_document(original.path()).unwrap();
        ConversationStore::copy_recovery_identity(original.path(), promoted.path()).unwrap();
        assert_eq!(
            ConversationStore::for_unsaved_document(promoted.path()).unwrap(),
            store
        );
        let other = tempfile::tempdir().unwrap();
        assert_ne!(
            ConversationStore::for_unsaved_document(other.path()).unwrap(),
            store
        );
    }

    #[test]
    fn malformed_legacy_history_is_preserved_without_creating_a_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let store = ConversationStore::for_project(dir.path());
        let legacy = dir.path().join(".auris-conversation.json");
        std::fs::write(&legacy, br#"{"turns":[{"user":"request"}]}"#).unwrap();
        assert!(store.migrate_legacy(&legacy).is_err());
        assert!(store.list().unwrap().is_empty());
        assert!(legacy.exists());
    }
}
