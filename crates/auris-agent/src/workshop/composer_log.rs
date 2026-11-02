//! A composer's tool calls, written beside its candidate so a run can be read afterwards.
//!
//! The conversation keeps only per-tool counts; this is the account of what was actually asked
//! and answered. `composer-log.jsonl` in the candidate's folder holds one JSON object per line:
//! the assignment, each call with its arguments, each result with its text, refused permissions,
//! the model's prose between calls and how the attempt ended. Long values are cut and the file
//! stops growing at [`LIMIT`], so a runaway candidate cannot fill the disk. It lives and dies
//! with the candidate's folder, so deleting the conversation deletes it. Writing is best effort:
//! a log that cannot be written never fails the music.

use serde_json::{Value, json};
use std::{collections::HashSet, io::Write, path::Path, time::Instant};

/// File name inside the candidate's folder.
pub(super) const FILE: &str = "composer-log.jsonl";
/// The file stops growing past this many bytes and says so once.
const LIMIT: u64 = 8 * 1024 * 1024;
/// Longest argument object kept whole, in bytes of JSON; a longer one is kept as cut text.
const ARGUMENTS: usize = 8 * 1024;
/// Longest result or prose text kept, in characters.
const TEXT: usize = 4 * 1024;

pub(super) struct Log {
    file: Option<std::fs::File>,
    started: Instant,
    written: u64,
    prose: String,
    calls: HashSet<String>,
    results: HashSet<String>,
}

impl Log {
    /// Appends to the candidate's log; a recovery continues the same file.
    pub(super) fn open(folder: &Path) -> Self {
        let path = folder.join(FILE);
        // A new candidate's folder is made by its first checkpoint, after the log opens.
        let _ = std::fs::create_dir_all(folder);
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let written = std::fs::metadata(&path).map_or(0, |m| m.len());
        Self {
            file,
            started: Instant::now(),
            written,
            prose: String::new(),
            calls: HashSet::new(),
            results: HashSet::new(),
        }
    }

    /// Records one worker event. Backends may announce a call more than once while
    /// streaming; each call and each result is written once per call ID.
    pub(super) fn observe(&mut self, event: &Value) {
        match event["event"].as_str().unwrap_or_default() {
            "text_delta" => {
                if self.prose.chars().count() < TEXT {
                    self.prose
                        .push_str(event["text"].as_str().unwrap_or_default());
                }
            }
            "call" => {
                let id = call_id(event);
                if self.calls.insert(id.clone()) {
                    self.flush_prose();
                    self.write(json!({"kind":"call","id":id,"tool":event["tool"],"args":arguments(&event["args"])}));
                }
            }
            "result" => {
                let id = call_id(event);
                if self.results.insert(id.clone()) {
                    self.write(json!({"kind":"result","id":id,"tool":event["tool"],"ok":event["ok"],"text":cut(event["text"].as_str().unwrap_or_default())}));
                }
            }
            "answer" => {
                self.flush_prose();
                self.write(
                    json!({"kind":"answer","text":cut(event["text"].as_str().unwrap_or_default())}),
                );
            }
            "error" => {
                self.flush_prose();
                self.write(json!({"kind":"error","message":event["message"]}));
            }
            "ended" => {
                self.flush_prose();
                self.write(json!({"kind":"ended"}));
            }
            _ => {}
        }
    }

    /// Records a host decision on a permission request.
    pub(super) fn permission(&mut self, tool: &str, allowed: bool) {
        self.write(json!({"kind":"permission","tool":tool,"allowed":allowed}));
    }

    /// Records anything the host itself decided, such as the assignment or a timeout.
    pub(super) fn note(&mut self, kind: &str, value: Value) {
        self.flush_prose();
        self.write(json!({"kind":kind,"value":value}));
    }

    fn flush_prose(&mut self) {
        let prose = std::mem::take(&mut self.prose);
        if !prose.trim().is_empty() {
            self.write(json!({"kind":"prose","text":cut(&prose)}));
        }
    }

    fn write(&mut self, mut entry: Value) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        entry["seconds"] = json!((self.started.elapsed().as_secs_f64() * 10.0).round() / 10.0);
        let mut line = entry.to_string();
        line.push('\n');
        let size = line.len() as u64;
        if self.written + size > LIMIT {
            let _ = writeln!(file, "{}", json!({"kind":"truncated","limit_bytes":LIMIT}));
            self.file = None;
            return;
        }
        if file.write_all(line.as_bytes()).is_err() {
            self.file = None;
            return;
        }
        self.written += size;
    }
}

impl Drop for Log {
    fn drop(&mut self) {
        self.flush_prose();
    }
}

fn call_id(event: &Value) -> String {
    match &event["call_id"] {
        Value::String(id) => id.clone(),
        Value::Null => event["tool"].as_str().unwrap_or_default().to_owned(),
        other => other.to_string(),
    }
}

fn arguments(value: &Value) -> Value {
    let text = value.to_string();
    if text.len() <= ARGUMENTS {
        value.clone()
    } else {
        json!(cut(&text))
    }
}

fn cut(text: &str) -> String {
    let count = text.chars().count();
    if count <= TEXT {
        return text.to_owned();
    }
    let kept: String = text.chars().take(TEXT).collect();
    format!("{kept}… ({} more characters)", count - TEXT)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(folder: &Path) -> Vec<Value> {
        std::fs::read_to_string(folder.join(FILE))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    #[test]
    fn calls_results_and_prose_are_written_once_in_order() {
        let folder = tempfile::tempdir().unwrap();
        let mut log = Log::open(folder.path());
        log.note("assignment", json!("write a song"));
        log.observe(&json!({"event":"text_delta","text":"Inspecting "}));
        log.observe(&json!({"event":"text_delta","text":"first."}));
        for _ in 0..2 {
            log.observe(&json!({"event":"call","call_id":"a","tool":"inspect_project","args":{}}));
        }
        log.observe(&json!({"event":"result","call_id":"a","tool":"inspect_project","ok":false,"text":"x".repeat(TEXT + 5)}));
        log.permission("cli_command", false);
        log.observe(&json!({"event":"answer","text":"done"}));
        drop(log);
        let lines = lines(folder.path());
        let kinds: Vec<_> = lines.iter().map(|l| l["kind"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "assignment",
                "prose",
                "call",
                "result",
                "permission",
                "answer"
            ]
        );
        assert_eq!(lines[1]["text"], "Inspecting first.");
        assert_eq!(lines[3]["ok"], false);
        assert!(
            lines[3]["text"]
                .as_str()
                .unwrap()
                .ends_with("(5 more characters)")
        );
        assert!(lines.iter().all(|l| l["seconds"].is_number()));
    }

    #[test]
    fn a_new_candidate_folder_is_made_for_its_log() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("1");
        Log::open(&folder).note("assignment", json!(1));
        assert_eq!(lines(&folder).len(), 1);
    }

    #[test]
    fn a_recovery_appends_and_the_file_stops_at_its_limit() {
        let folder = tempfile::tempdir().unwrap();
        Log::open(folder.path()).note("assignment", json!(1));
        let mut log = Log::open(folder.path());
        log.note("recovery", json!(2));
        assert_eq!(lines(folder.path()).len(), 2);
        log.written = LIMIT - 10;
        log.note("big", json!("y".repeat(100)));
        log.note("after", json!(3));
        let lines = lines(folder.path());
        assert_eq!(lines.last().unwrap()["kind"], "truncated");
        assert_eq!(lines.len(), 3);
    }
}
