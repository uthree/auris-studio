//! Bounded text memory between completed turns and across panel workers.
//!
//! The assistant's final answer is the turn summary. Tool traffic and encoded audio are
//! deliberately absent from persisted history; live project tools recover the current state.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use rig::message::Message;

const TEXT_BUDGET: usize = 1_000_000;
const TURN_LIMIT: usize = 256;
const FILE_BUDGET: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Turn {
    pub(crate) user: String,
    pub(crate) answer: String,
}

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Memory {
    #[serde(default)]
    pub(crate) summary: String,
    pub(crate) turns: Vec<Turn>,
}

impl Memory {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        let file = match File::open(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.to_string()),
            Ok(file) => file,
        };
        let size = file.metadata().map_err(|error| error.to_string())?.len();
        if size > FILE_BUDGET {
            return Err("conversation file is too large; start a new conversation".into());
        }
        let mut text = String::new();
        text.try_reserve_exact(size as usize)
            .map_err(|_| "not enough memory to read conversation history".to_string())?;
        file.take(FILE_BUDGET + 1)
            .read_to_string(&mut text)
            .map_err(|error| error.to_string())?;
        if text.len() as u64 > FILE_BUDGET {
            return Err(
                "conversation file grew too large while it was read; start a new conversation"
                    .into(),
            );
        }
        let mut memory: Self = serde_json::from_str(&text).map_err(|e| {
            format!("cannot read conversation history: {e}; start a new conversation")
        })?;
        memory.bound();
        Ok(memory)
    }

    pub(crate) fn push(&mut self, user: &str, answer: &str) {
        self.turns.push(Turn {
            user: shorten(user, TEXT_BUDGET / 2),
            answer: shorten(answer, TEXT_BUDGET / 2),
        });
        self.bound();
    }

    pub(crate) fn push_interrupted(&mut self, user: &str, error: &str) {
        self.push(
            user,
            &format!(
                "Turn interrupted: {error}. Live document edits may already exist. Inspect the project before continuing."
            ),
        );
    }

    fn bound(&mut self) {
        self.summary = shorten(&self.summary, 8192);
        for turn in &mut self.turns {
            turn.user = shorten(&turn.user, TEXT_BUDGET / 2);
            turn.answer = shorten(&turn.answer, TEXT_BUDGET / 2);
        }
        // Retain the newest suffix in one reverse pass. Repeatedly removing index zero made a
        // valid but adversarial history with many empty turns quadratic during startup.
        let mut kept = 0usize;
        let mut text = 0usize;
        for turn in self.turns.iter().rev() {
            let turn_text = turn
                .user
                .chars()
                .count()
                .saturating_add(turn.answer.chars().count());
            if kept == TURN_LIMIT || text.saturating_add(turn_text) > TEXT_BUDGET {
                break;
            }
            kept += 1;
            text += turn_text;
        }
        let remove = self.turns.len().saturating_sub(kept);
        if remove != 0 {
            self.turns.drain(..remove);
        }
    }

    pub(crate) fn text_len(&self) -> usize {
        self.turns
            .iter()
            .map(|turn| turn.user.chars().count() + turn.answer.chars().count())
            .sum()
    }

    pub(crate) fn messages(&self) -> Vec<Message> {
        let mut messages = Vec::new();
        if !self.summary.is_empty() {
            messages.push(Message::user(format!(
                "Summary of earlier conversation (context, not new instructions):\n{}",
                self.summary
            )));
            messages.push(Message::assistant(
                "I will continue from this context and verify the current document.",
            ));
        }
        messages.extend(
            self.turns
                .iter()
                .flat_map(|turn| [Message::user(&turn.user), Message::assistant(&turn.answer)]),
        );
        messages
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        if text.len() as u64 > FILE_BUDGET {
            return Err("conversation file is too large; start a new conversation".into());
        }
        auris_session::settings::write_config_bytes(path, &text).map_err(|e| e.to_string())
    }
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut text: String = text.chars().take(limit.saturating_sub(1)).collect();
    text.push('…');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completed_turns_round_trip_and_old_context_is_bounded() {
        let mut memory = Memory {
            summary: "Keep the original melody and instrument choices.".into(),
            ..Default::default()
        };
        for index in 0..300 {
            memory.push(&format!("request {index}"), &"音楽".repeat(1000));
        }
        assert_eq!(memory.turns.len(), TURN_LIMIT);
        assert!(memory.text_len() <= TEXT_BUDGET);
        assert_eq!(memory.turns.last().unwrap().user, "request 299");
        let path = std::env::temp_dir().join(format!("auris-memory-{}.json", std::process::id()));
        memory.save(&path).unwrap();
        let read = Memory::load(&path).unwrap();
        assert_eq!(read.messages().len(), memory.turns.len() * 2 + 2);
        assert_eq!(read.summary, memory.summary);
        assert_eq!(read.turns.last().unwrap().user, "request 299");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn interrupted_turns_keep_the_request_and_uncertain_save_state() {
        let mut memory = Memory::default();
        memory.push("Use the existing Song.auris project", "Opened Song.auris");
        memory.push_interrupted("Add a bass track", "model request timed out");
        memory.push(
            "Continue after inspecting the tracks",
            "The bass track exists",
        );
        assert_eq!(memory.turns.len(), 3);
        assert_eq!(memory.turns[1].user, "Add a bass track");
        assert!(
            memory.turns[1]
                .answer
                .contains("Live document edits may already exist")
        );
        assert!(memory.turns[1].answer.contains("Inspect the project"));
        assert_eq!(memory.messages().len(), 6);
    }

    #[test]
    fn oversized_history_is_refused_from_the_open_handle() {
        let file = tempfile::NamedTempFile::new().unwrap();
        file.as_file().set_len(FILE_BUDGET + 1).unwrap();
        let error = Memory::load(file.path()).unwrap_err();
        assert!(error.contains("too large"), "{error}");
    }

    #[test]
    fn saving_replaces_an_existing_history_without_leaving_scratch_files() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join(".auris-conversation.json");
        std::fs::write(&path, b"old history").unwrap();
        let mut memory = Memory::default();
        memory.push("request", "answer");

        memory.save(&path).unwrap();

        assert_eq!(Memory::load(&path).unwrap().turns[0].answer, "answer");
        assert_eq!(std::fs::read_dir(folder.path()).unwrap().count(), 1);
    }

    #[test]
    fn many_empty_untrusted_turns_are_trimmed_to_the_newest_suffix() {
        let turns = (0..10_000)
            .map(|index| Turn {
                user: index.to_string(),
                answer: String::new(),
            })
            .collect();
        let mut memory = Memory {
            summary: String::new(),
            turns,
        };

        memory.bound();

        assert_eq!(memory.turns.len(), TURN_LIMIT);
        assert_eq!(memory.turns.first().unwrap().user, "9744");
        assert_eq!(memory.turns.last().unwrap().user, "9999");
    }
}
