//! A per-attempt record of how a workshop candidate was composed.
//!
//! The workshop shows only a candidate's last error; this is the account behind it. Each attempt
//! writes `logs/attempt-<unix ms>.jsonl` in the candidate's own folder, one JSON object a line:
//! what the composer said and thought, every tool it called with its arguments and answer, each
//! command the host executed with its duration and whether the document moved, every permission
//! decision, and how the attempt ended. It stays beside the candidate, so it travels with the
//! project on Save As and is removed with it. Logging never fails a candidate: a log that cannot
//! be written is reported once and then skipped.

use serde_json::{Value, json};
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::{Instant, SystemTime, UNIX_EPOCH},
};

/// Longest string kept in one field; a full `inspect_project` answer can run to megabytes.
const MAX_FIELD_BYTES: usize = 16 * 1024;
/// Total bytes one attempt may write before the rest is dropped with a marker.
const MAX_FILE_BYTES: usize = 32 * 1024 * 1024;

pub(crate) struct GenerationLog {
    file: Option<BufWriter<File>>,
    path: Option<PathBuf>,
    started: Instant,
    written: usize,
    /// Streamed text and reasoning, written as one entry when something else happens.
    text: String,
    reasoning: String,
}

impl GenerationLog {
    /// Opens a new attempt log in `folder/logs`.
    pub(crate) fn create(folder: &Path) -> Self {
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let path = folder.join("logs").join(format!("attempt-{millis}.jsonl"));
        let file = std::fs::create_dir_all(folder.join("logs"))
            .and_then(|()| File::create(&path))
            .inspect_err(|error| {
                log::warn!("generation log {} not written: {error}", path.display())
            })
            .ok()
            .map(BufWriter::new);
        Self {
            path: file.is_some().then_some(path),
            file,
            started: Instant::now(),
            written: 0,
            text: String::new(),
            reasoning: String::new(),
        }
    }

    /// Where this attempt is being written, when it is.
    pub(crate) fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Writes one entry of `kind` with `fields` merged in.
    pub(crate) fn record(&mut self, kind: &str, fields: Value) {
        self.flush_streams();
        self.write(kind, fields);
    }

    /// Records an event from the composer's worker, joining streamed deltas into whole entries.
    pub(crate) fn worker_event(&mut self, event: &Value) {
        let text = event["text"].as_str().unwrap_or_default();
        match event["event"].as_str().unwrap_or_default() {
            "text_delta" => {
                if !self.reasoning.is_empty() {
                    self.flush_streams();
                }
                self.text.push_str(text);
            }
            "reasoning_delta" => {
                if !self.text.is_empty() {
                    self.flush_streams();
                }
                self.reasoning.push_str(text);
            }
            // The host's own handling of these is recorded with its outcome.
            "edit" | "permission" => {}
            kind => {
                let mut fields = event.clone();
                if let Some(fields) = fields.as_object_mut() {
                    fields.remove("event");
                }
                self.record(&format!("worker.{kind}"), fields);
            }
        }
    }

    fn flush_streams(&mut self) {
        for (kind, text) in [
            ("model.reasoning", std::mem::take(&mut self.reasoning)),
            ("model.text", std::mem::take(&mut self.text)),
        ] {
            if !text.is_empty() {
                self.write(kind, json!({ "text": text }));
            }
        }
    }

    fn write(&mut self, kind: &str, fields: Value) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let mut entry = json!({
            "t_ms": self.started.elapsed().as_millis() as u64,
            "kind": kind,
        });
        if let (Some(entry), Value::Object(fields)) = (entry.as_object_mut(), clip(fields)) {
            entry.extend(fields);
        }
        let mut line = entry.to_string();
        line.push('\n');
        let full = self.written + line.len() > MAX_FILE_BYTES;
        if full {
            line = json!({"t_ms": self.started.elapsed().as_millis() as u64,
                "kind": "log.truncated", "limit_bytes": MAX_FILE_BYTES})
            .to_string()
                + "\n";
        }
        self.written += line.len();
        if let Err(error) = file.write_all(line.as_bytes()).and_then(|()| file.flush()) {
            log::warn!("generation log stopped: {error}");
            self.file = None;
        }
        if full {
            self.file = None;
        }
    }
}

impl Drop for GenerationLog {
    fn drop(&mut self) {
        self.flush_streams();
    }
}

/// Shortens every string longer than [`MAX_FIELD_BYTES`], saying how long it was.
fn clip(value: Value) -> Value {
    match value {
        Value::String(text) if text.len() > MAX_FIELD_BYTES => {
            let mut end = MAX_FIELD_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            Value::String(format!(
                "{}… [{} of {} bytes]",
                &text[..end],
                end,
                text.len()
            ))
        }
        Value::Array(items) => Value::Array(items.into_iter().map(clip).collect()),
        Value::Object(fields) => {
            Value::Object(fields.into_iter().map(|(k, v)| (k, clip(v))).collect())
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(log: &GenerationLog) -> Vec<Value> {
        std::fs::read_to_string(log.path().unwrap())
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn streamed_text_is_joined_and_long_fields_are_clipped() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = GenerationLog::create(dir.path());
        log.record("attempt.start", json!({"model": "m"}));
        for delta in ["Writ", "ing the ", "bass"] {
            log.worker_event(&json!({"event": "text_delta", "text": delta}));
        }
        log.worker_event(&json!({"event": "call", "tool": "notes", "args": {"track": 1}}));
        log.worker_event(&json!({"event": "edit", "command": {"action": "notes"}}));
        log.record(
            "host.edit",
            json!({"result": "x".repeat(MAX_FIELD_BYTES + 10)}),
        );
        log.worker_event(&json!({"event": "reasoning_delta", "text": "done?"}));
        let path = log.path().unwrap().to_owned();
        // Text still streaming when the attempt ends is not lost.
        drop(log);
        let lines: Vec<Value> = std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let kinds: Vec<_> = lines.iter().map(|l| l["kind"].as_str().unwrap()).collect();
        // The edit request itself is left to the host, which records it with its outcome.
        assert_eq!(
            kinds,
            [
                "attempt.start",
                "model.text",
                "worker.call",
                "host.edit",
                "model.reasoning"
            ]
        );
        assert_eq!(lines[1]["text"], "Writing the bass");
        assert_eq!(lines[2]["args"]["track"], 1);
        assert!(lines[3]["result"].as_str().unwrap().ends_with(&format!(
            "[{MAX_FIELD_BYTES} of {} bytes]",
            MAX_FIELD_BYTES + 10
        )));
    }

    #[test]
    fn reasoning_flushes_when_the_model_starts_to_answer() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = GenerationLog::create(dir.path());
        log.worker_event(&json!({"event": "reasoning_delta", "text": "think"}));
        log.worker_event(&json!({"event": "text_delta", "text": "say"}));
        log.record("attempt.end", json!({"ok": true}));
        let kinds: Vec<_> = entries(&log)
            .iter()
            .map(|l| l["kind"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(kinds, ["model.reasoning", "model.text", "attempt.end"]);
    }
}
