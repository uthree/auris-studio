//! Agent ownership of ordinary undo entries, without keeping a transaction open.

use super::*;
use crate::live_agent::HistoryOperation;

pub(super) struct AgentRequest {
    id: u64,
    revision: u64,
    interrupted: bool,
}

impl Session {
    /// Starts one user request's edit group; the host must finish it on completion or stop.
    /// Playback and ordinary human gestures remain immediate between agent tool calls.
    pub fn begin_agent_request(&mut self) {
        self.last_record = None;
        self.agent_request_serial = self.agent_request_serial.wrapping_add(1);
        self.agent_request = Some(AgentRequest {
            id: self.agent_request_serial,
            revision: self.revision,
            interrupted: false,
        });
    }

    /// Finishes the current agent request without recording an empty undo entry.
    pub fn end_agent_request(&mut self) {
        self.agent_request = None;
        self.last_record = None;
    }

    /// Run an agent's command under a phase lock, withdrawing it if it changed locked material.
    ///
    /// Withdrawn means gone: the document, the dirty flag and the undo history are as they were
    /// before the call, so neither the screen nor Undo shows an edit the agent was not allowed to
    /// make. The error the agent reads names what changed and the phase that may change it.
    ///
    /// Read-only commands skip the comparison. So does the agent's own undo and redo, which
    /// moves history in a way a history mark cannot put back; the lock is on new
    /// edits, not on stepping through ones already made.
    pub fn agent_command_locked(
        &mut self,
        command: crate::live_agent::Command,
        lock: Option<&crate::phase::Lock>,
        plugin_paths: &[std::path::PathBuf],
    ) -> Result<String, String> {
        let Some(lock) = lock.filter(|_| {
            command.is_mutating() && !matches!(command, crate::live_agent::Command::History { .. })
        }) else {
            return self.agent_command_with_plugin_paths(command, plugin_paths);
        };
        let printed = crate::phase::print(&self.project, lock);
        let before = self.project.clone();
        let dirty = self.dirty;
        let mark = self.history.mark();
        let result = self.agent_command_with_plugin_paths(command, plugin_paths);
        let changes =
            crate::phase::changes(&printed, &crate::phase::print(&self.project, lock), &before);
        if changes.is_empty() {
            return result;
        }
        self.history.rollback(mark);
        self.restore_history_project(Edit::ExternalChanges, before);
        self.dirty = dirty;
        Err(crate::phase::refusal(lock, &changes))
    }

    pub(crate) fn with_agent_history<T>(
        &mut self,
        apply: impl FnOnce(&mut Self) -> Result<T, String>,
    ) -> Result<T, String> {
        if let Some(request) = &mut self.agent_request {
            self.last_record = None;
            request.interrupted |= request.revision != self.revision;
            self.history.recording_agent = Some(crate::history::AgentGroup {
                id: request.id,
                interrupted: request.interrupted,
            });
        }
        // An open human gesture owns its transaction; an agent must not join or close it.
        if self.transaction.is_some() && self.history.recording_agent.is_some() {
            self.history.recording_agent = None;
            return Err(
                "Finish the human editing gesture before applying an agent operation".into(),
            );
        }
        let result = apply(self);
        self.history.recording_agent = None;
        if self.agent_request.is_some() {
            self.last_record = None;
        }
        if let Some(request) = &mut self.agent_request {
            request.revision = self.revision;
        }
        result
    }

    pub(crate) fn agent_history(&mut self, operation: HistoryOperation) -> Result<String, String> {
        let (redo, group, expected) = match operation {
            HistoryOperation::Read => return Ok(self.agent_history_report()),
            HistoryOperation::Undo {
                group,
                expected_revision,
            } => (false, group, expected_revision),
            HistoryOperation::Redo {
                group,
                expected_revision,
            } => (true, group, expected_revision),
        };
        if expected != self.revision {
            return Err(
                "Document changed; read history again before reversing an agent request".into(),
            );
        }
        if self.transaction.is_some() {
            return Err("A human gesture owns the document".into());
        }
        if self.history.agent_target(redo) != Some(group) {
            return Err("Only the latest uninterrupted agent group can be reversed; human edits are a history boundary".into());
        }
        let applied = if redo { self.redo() } else { self.undo() };
        if applied.is_none() {
            return Err("The history step could not be applied".into());
        }
        Ok(self.agent_history_report())
    }

    fn agent_history_report(&self) -> String {
        serde_json::json!({"revision":self.revision,"undo":self.history.agent_summary(false),
            "redo":self.history.agent_summary(true),"gesture_open":self.transaction.is_some(),
            "usage":"Use an available_group and this numeric revision to undo/redo one complete user request. Never reconstruct old notes. Only the latest uninterrupted agent group is reversible; human edits are boundaries. History is session-local and does not save the document."}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SessionOptions, live_agent::Command};
    use serde_json::Value;

    fn session() -> (Session, TrackId) {
        let mut session = Session::new(SessionOptions::headless()).unwrap();
        let track = session.add_default_instrument_track("Original").unwrap();
        session.forget_history();
        (session, track)
    }

    fn rename(session: &mut Session, track: TrackId, name: &str) {
        session
            .agent_command(Command::RenameTrack {
                track: track.0,
                name: name.into(),
            })
            .unwrap();
    }

    fn read(session: &mut Session) -> Value {
        serde_json::from_str(&session.agent_history(HistoryOperation::Read).unwrap()).unwrap()
    }

    #[test]
    fn a_request_is_one_ordinary_undo_step_and_redo_restores_every_edit() {
        let (mut session, track) = session();
        let before = session.project().clone();
        session.begin_agent_request();
        rename(&mut session, track, "First edit");
        session
            .agent_command(Command::AddTrack {
                name: "Second edit".into(),
                kind: crate::live_agent::TrackKind::Instrument,
            })
            .unwrap();
        session.end_agent_request();
        let after = session.project().clone();
        session.undo().unwrap();
        assert_eq!(session.project(), &before);
        assert!(!session.can_undo());
        session.redo().unwrap();
        assert_eq!(session.project(), &after);
    }

    #[test]
    fn history_reverses_only_the_latest_request_and_does_not_add_its_own_step() {
        let (mut session, track) = session();
        session.begin_agent_request();
        rename(&mut session, track, "First request");
        session.end_agent_request();
        let first = session.project().clone();
        session.begin_agent_request();
        rename(&mut session, track, "Second request");
        rename(&mut session, track, "More of second");
        session.end_agent_request();
        let second = session.project().clone();
        let history = read(&mut session);
        let group = history["undo"]["available_group"].as_u64().unwrap();
        session.begin_agent_request();
        session
            .agent_command(Command::History {
                operation: HistoryOperation::Undo {
                    group,
                    expected_revision: history["revision"].as_u64().unwrap(),
                },
            })
            .unwrap();
        session.end_agent_request();
        assert_eq!(session.project(), &first);
        let history = read(&mut session);
        assert_eq!(history["redo"]["available_group"], group);
        session
            .agent_history(HistoryOperation::Redo {
                group,
                expected_revision: history["revision"].as_u64().unwrap(),
            })
            .unwrap();
        assert_eq!(session.project(), &second);
    }

    #[test]
    fn stale_reversal_and_intervening_human_edits_preserve_the_document() {
        let (mut session, track) = session();
        session.begin_agent_request();
        rename(&mut session, track, "Agent");
        session.end_agent_request();
        let history = read(&mut session);
        let group = history["undo"]["available_group"].as_u64().unwrap();
        session.add_default_instrument_track("Human").unwrap();
        let before = session.project().clone();
        assert!(
            session
                .agent_history(HistoryOperation::Undo {
                    group,
                    expected_revision: history["revision"].as_u64().unwrap()
                })
                .is_err()
        );
        let current = read(&mut session);
        assert!(current["undo"]["available_group"].is_null());
        assert!(
            session
                .agent_history(HistoryOperation::Undo {
                    group,
                    expected_revision: current["revision"].as_u64().unwrap()
                })
                .is_err()
        );
        assert_eq!(session.project(), &before);
    }

    #[test]
    fn human_edits_between_tools_make_the_entire_request_unavailable_to_the_agent() {
        let (mut session, track) = session();
        session.begin_agent_request();
        rename(&mut session, track, "Agent before");
        session
            .add_default_instrument_track("Human interruption")
            .unwrap();
        rename(&mut session, track, "Agent after");
        session.end_agent_request();
        let history = read(&mut session);
        assert!(history["undo"]["available_group"].is_null());
        assert_eq!(history["undo"]["entries"][0]["interrupted"], true);
    }

    #[test]
    fn empty_or_cancelled_requests_create_no_step_and_open_gestures_cannot_be_joined() {
        let (mut session, track) = session();
        session.begin_agent_request();
        read(&mut session);
        session.end_agent_request();
        assert!(!session.can_undo());
        session.begin_transaction(Edit::ExternalChanges);
        session.begin_agent_request();
        assert!(
            session
                .agent_command(Command::RenameTrack {
                    track: track.0,
                    name: "Rejected".into()
                })
                .is_err()
        );
        session.end_agent_request();
        session.cancel_transaction();
        assert_eq!(session.project().track(track).unwrap().name, "Original");
        assert!(!session.can_undo());
    }
}
