//! Runtime persistence and trusted tool observations for a conversation's production record.
use super::*;
use auris_session::production_record::{Operation, Record};
use serde_json::{Value, json};

#[derive(Default)]
pub(super) struct State {
    history: Option<HistoryGeneration>,
    record: Record,
    document: Value,
}

/// The record as a tool answers with it, with the current phase's instructions when asked for.
///
/// The instructions go on after the record is paged, not inside it: a long record is cut into
/// `read_report` pages, and the one text a model must not miss would otherwise be behind them.
/// Only entering a phase and reading the record carry them; every `put` repeating several
/// kilobytes the model already has would cost more than it tells.
fn record_view(record: &Record, document: &Value, instructions: bool) -> Result<Value, String> {
    let text = toolbox::present_domain_report(&record.view(document).to_string())?;
    let mut view: Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if instructions && let Some(state) = &record.phase {
        let mut text =
            toolbox::composition_workflows::instructions(state.phase, state.writing_order);
        if let Some(style) = toolbox::styles::guidance(&document["style"], state.phase) {
            text.push_str(
                "
This song's style
",
            );
            text.push_str(&style);
        }
        view["phase_instructions"] = text.into();
    }
    Ok(view)
}

/// The record saved beside a conversation's history, or an empty one when there is none.
///
/// Under the lock the record's own save renames under, since on Windows that rename fails for
/// as long as another thread has the file open.
fn read_record(path: &Path) -> Result<Record, String> {
    match auris_session::settings::read_published_file(path, 1024 * 1024) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Record::default()),
        Err(e) if e.kind() == std::io::ErrorKind::FileTooLarge => {
            Err("Production record exceeds 1 MiB".into())
        }
        Err(e) => Err(e.to_string()),
    }
}

impl Bridge {
    pub(crate) fn bind_production_history(
        &self,
        history: Option<&HistoryGeneration>,
    ) -> Result<(), String> {
        self.0
            .production
            .lock()
            .map_err(|_| "Production record lock failed")?
            .history = history.cloned();
        Ok(())
    }

    fn with_production<T>(
        &self,
        operation: impl FnOnce(&mut State) -> Result<T, String>,
        write: bool,
    ) -> Result<T, String> {
        self.history(|| {
            let mut state = self
                .0
                .production
                .lock()
                .map_err(|_| "Production record lock failed")?;
            let binding = state.history.clone();
            let _file_lock = binding
                .as_ref()
                .map(|h| history_file_lock(&h.path))
                .transpose()?;
            if let Some(history) = &binding {
                if read_history_generation(&history.path)? != history.generation {
                    return Err("Conversation was reset; production record is stale".into());
                }
                state.record = read_record(&history.path.with_extension("production.json"))?;
            }
            let result = operation(&mut state)?;
            if write && let Some(history) = binding {
                let bytes = serde_json::to_vec(&state.record).map_err(|e| e.to_string())?;
                auris_session::settings::write_config_bytes(
                    &history.path.with_extension("production.json"),
                    &bytes,
                )
                .map_err(|e| e.to_string())?;
            }
            Ok(result)
        })
    }

    pub(crate) fn production_record(&self, operation: Operation) -> Result<Value, String> {
        let write = !matches!(operation, Operation::Read);
        let instructions = matches!(operation, Operation::Read | Operation::EnterPhase { .. });
        self.with_production(
            |state| {
                if !state.document["revision"].is_string() {
                    return Err(
                        "Call inspect_project before reading or updating production_record".into(),
                    );
                }
                state
                    .record
                    .update_with_document(operation, &state.document)?;
                let mut view = record_view(&state.record, &state.document, instructions)?;
                view["persistence"] = json!(if state.history.is_some() {
                    "conversation"
                } else {
                    "worker_lifetime"
                });
                Ok(view)
            },
            write,
        )
    }

    /// The record as it stands, for a host that keeps it beside the document it describes.
    pub(crate) fn production_snapshot(&self) -> Result<Record, String> {
        self.with_production(|state| Ok(state.record.clone()), false)
    }

    /// Start from a record a host kept, before the model's first turn.
    pub(crate) fn seed_production(&self, record: Record) -> Result<(), String> {
        self.with_production(
            |state| {
                state.record = record;
                Ok(())
            },
            false,
        )
    }

    /// What the phase in force keeps fixed, for the owner to hold an edit to.
    pub(crate) fn phase_lock(&self) -> Result<Option<auris_session::phase::Lock>, String> {
        self.with_production(|state| Ok(state.record.lock()), false)
    }

    pub(crate) fn inspected_production_document(&self, document: &mut Value) -> Result<(), String> {
        let view = self.with_production(
            |state| {
                state.document = document.clone();
                record_view(&state.record, &state.document, false)
            },
            false,
        )?;
        // Per-bar digests are for the record to compare, not for the model to read: several
        // kilobytes of hex in every inspection would crowd out what the model needs.
        if let Some(facts) = document["phase_facts"].as_object_mut() {
            facts.remove("score_bars");
        }
        document["production_record"] = view;
        Ok(())
    }

    /// Keep the inspected document's style current after an edit that chose one, so the next
    /// phase instructions follow it without the model having to inspect the project again.
    pub(crate) fn observed_style(&self, style: Value) -> Result<(), String> {
        self.with_production(
            |state| {
                if state.document.is_object() {
                    state.document["style"] = style;
                }
                Ok(())
            },
            false,
        )
    }

    pub(crate) fn observe_production(
        &self,
        tool: &str,
        revision: u64,
        range: Option<(u64, u64)>,
        track: Option<u64>,
        result: Value,
    ) -> Result<(), String> {
        self.with_production(
            |state| {
                state
                    .record
                    .observe(&state.document, tool, revision, range, track, result);
                Ok(())
            },
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_save_succeeds_while_another_thread_reads_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memory.production.json");
        let read = path.clone();
        let bytes = serde_json::to_vec(&Record::default()).unwrap();
        crate::tests::assert_writes_survive_a_reader(
            move || read_record(&read).map(drop),
            |_| {
                auris_session::settings::write_config_bytes(&path, &bytes)
                    .map_err(|e| e.to_string())
            },
        );
    }
    #[test]
    fn the_phase_instructions_end_with_the_documents_style() {
        let mut record = Record::default();
        record
            .update(
                serde_json::from_value(
                    json!({"kind":"enter_phase","phase":"score","reason":"start"}),
                )
                .unwrap(),
            )
            .unwrap();
        let styled = json!({"revision":"r","style":{"genre":"jazz","usage":"loop_bgm"}});
        let text = record_view(&record, &styled, true).unwrap()["phase_instructions"]
            .as_str()
            .unwrap()
            .to_owned();
        let style = text
            .split_once(
                "This song's style
",
            )
            .unwrap()
            .1;
        assert!(style.starts_with("Genre: Jazz."), "{style}");
        assert!(
            style.contains(
                "
Usage: Loop background music."
            ),
            "{style}"
        );
        let plain = record_view(&record, &json!({"revision":"r"}), true).unwrap();
        assert!(
            !plain["phase_instructions"]
                .as_str()
                .unwrap()
                .contains("This song's style")
        );
    }

    #[test]
    fn large_records_use_bounded_report_pages_without_dropping_stored_context() {
        let mut record = Record::default();
        for index in 0..12 {
            record
                .update(
                    serde_json::from_value(json!({"kind":"put","entry":{
                "key":format!("decision_{index}"),"category":"decision","source":"agent_assumption",
                "basis":"explicit development choice","text":"x".repeat(1024)}}))
                    .unwrap(),
                )
                .unwrap();
        }
        let view = record_view(&record, &json!({"revision":"today"}), false).unwrap();
        assert!(view["report_id"].is_string());
        assert!(view.to_string().len() <= 8192);
        assert_eq!(record.entries.len(), 12);

        // The phase's instructions arrive whole beside a paged record, never inside its pages.
        record
            .update(serde_json::from_value(json!({"kind":"set_roles","foreground":[1]})).unwrap())
            .unwrap();
        record
            .update(
                serde_json::from_value(
                    json!({"kind":"enter_phase","phase":"arrangement","reason":"dress it"}),
                )
                .unwrap(),
            )
            .unwrap();
        let view = record_view(&record, &json!({"revision":"today"}), true).unwrap();
        assert!(view["report_id"].is_string());
        let instructions = view["phase_instructions"].as_str().unwrap();
        assert!(instructions.starts_with("Arrangement phase"));
        assert!(instructions.contains("Techniques"));
        let quiet = record_view(&record, &json!({"revision":"today"}), false).unwrap();
        assert!(quiet.get("phase_instructions").is_none());
    }
    #[test]
    fn records_resume_and_reset_rejects_a_late_writer() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.json");
        let (_, generation) = HistoryGeneration::load(&path, false).unwrap();
        let (bridge, _, _) = test_bridge();
        bridge.bind_production_history(Some(&generation)).unwrap();
        bridge
            .inspected_production_document(&mut json!({"revision":"one"}))
            .unwrap();
        bridge.production_record(serde_json::from_value(json!({"kind":"put","entry":{
            "key":"motif","category":"motif","source":"agent_assumption","basis":"opening phrase","text":"keep the rising third"}})).unwrap()).unwrap();
        bridge
            .production_record(Operation::EnterPhase {
                phase: auris_session::production_record::Phase::Score,
                reason: "new song".into(),
                writing_order: Some(auris_session::production_record::WritingOrder::MelodyFirst),
            })
            .unwrap();
        let (resumed, _, _) = test_bridge();
        resumed.bind_production_history(Some(&generation)).unwrap();
        resumed
            .inspected_production_document(&mut json!({"revision":"two"}))
            .unwrap();
        assert_eq!(
            resumed.production_record(Operation::Read).unwrap()["entries"][0]["entry"]["text"],
            "keep the rising third"
        );
        assert_eq!(
            resumed.production_record(Operation::Read).unwrap()["phase"]["current"],
            "score"
        );
        assert_eq!(
            resumed.production_record(Operation::Read).unwrap()["phase"]["writing_order"],
            "melody_first"
        );
        clear_history(&path).unwrap();
        assert!(
            bridge
                .production_record(Operation::Remove {
                    key: "motif".into()
                })
                .is_err()
        );
        assert!(!path.with_extension("production.json").exists());
    }
}
