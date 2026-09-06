//! Bounded text memory between completed turns and across panel subprocesses.
//!
//! The assistant's final answer is the turn summary. Tool traffic and encoded audio are
//! deliberately absent from persisted history; live project tools recover the current state.

use std::path::Path;

use rig::message::Message;

const TEXT_BUDGET: usize = 24_000;
const TURN_LIMIT: usize = 24;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Turn {
    pub(crate) user: String,
    pub(crate) answer: String,
}

#[derive(Default, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) struct Memory {
    pub(crate) turns: Vec<Turn>,
}

impl Memory {
    pub(crate) fn load(path: &Path) -> Result<Self, String> {
        match std::fs::metadata(path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(error.to_string()),
            Ok(metadata) if metadata.len() > 2 * 1024 * 1024 => {
                return Err("conversation file is too large; start a new conversation".into());
            }
            Ok(_) => {}
        }
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
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

    fn bound(&mut self) {
        for turn in &mut self.turns {
            turn.user = shorten(&turn.user, TEXT_BUDGET / 2);
            turn.answer = shorten(&turn.answer, TEXT_BUDGET / 2);
        }
        while self.turns.len() > TURN_LIMIT || self.text_len() > TEXT_BUDGET {
            self.turns.remove(0);
        }
    }

    fn text_len(&self) -> usize {
        self.turns
            .iter()
            .map(|turn| turn.user.chars().count() + turn.answer.chars().count())
            .sum()
    }

    pub(crate) fn messages(&self) -> Vec<Message> {
        self.turns
            .iter()
            .flat_map(|turn| [Message::user(&turn.user), Message::assistant(&turn.answer)])
            .collect()
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let temporary = path.with_extension(format!("{}.saving", std::process::id()));
        std::fs::write(&temporary, text).map_err(|e| e.to_string())?;
        std::fs::rename(&temporary, path).map_err(|e| e.to_string())
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
        let mut memory = Memory::default();
        for index in 0..40 {
            memory.push(&format!("request {index}"), &"音楽".repeat(1000));
        }
        assert!(memory.turns.len() < TURN_LIMIT);
        assert!(memory.text_len() <= TEXT_BUDGET);
        assert_eq!(memory.turns.last().unwrap().user, "request 39");
        let path = std::env::temp_dir().join(format!("auris-memory-{}.json", std::process::id()));
        memory.save(&path).unwrap();
        let read = Memory::load(&path).unwrap();
        assert_eq!(read.messages().len(), memory.turns.len() * 2);
        assert_eq!(read.turns.last().unwrap().user, "request 39");
        std::fs::remove_file(path).unwrap();
    }
}
