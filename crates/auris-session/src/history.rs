//! Undo/redo over whole-project snapshots.
//!
//! A [`Project`] holds ids, numbers and note lists — never audio samples — so a clone costs a
//! few kilobytes even for a large arrangement. Snapshotting the whole document is therefore
//! affordable, and it removes a whole class of bugs that command-based undo suffers from, where
//! one edit path forgets to record its inverse.

use auris_core::Project;

/// A bounded undo/redo stack of project snapshots.
pub struct History {
    past: Vec<Snapshot>,
    future: Vec<Snapshot>,
    limit: usize,
}

struct Snapshot {
    label: String,
    project: Project,
}

impl Default for History {
    fn default() -> Self {
        Self::new(64)
    }
}

impl History {
    /// A history holding at most `limit` undo steps.
    pub fn new(limit: usize) -> Self {
        Self {
            past: Vec::new(),
            future: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Records the state *before* an edit described by `label`.
    ///
    /// Call this immediately before mutating the project. Recording a new edit discards the
    /// redo stack, which is what every editor does after diverging from an undone branch.
    pub fn push(&mut self, label: impl Into<String>, project: &Project) {
        self.future.clear();
        self.past.push(Snapshot {
            label: label.into(),
            project: project.clone(),
        });
        if self.past.len() > self.limit {
            self.past.remove(0);
        }
    }

    /// Steps back, returning the project state to restore.
    ///
    /// `current` is the live project, which moves onto the redo stack.
    pub fn undo(&mut self, current: &Project) -> Option<Project> {
        let snapshot = self.past.pop()?;
        self.future.push(Snapshot {
            label: snapshot.label.clone(),
            project: current.clone(),
        });
        Some(snapshot.project)
    }

    /// Steps forward, returning the project state to restore.
    pub fn redo(&mut self, current: &Project) -> Option<Project> {
        let snapshot = self.future.pop()?;
        self.past.push(Snapshot {
            label: snapshot.label.clone(),
            project: current.clone(),
        });
        Some(snapshot.project)
    }

    /// Label of the edit that [`Self::undo`] would reverse.
    pub fn undo_label(&self) -> Option<&str> {
        self.past.last().map(|s| s.label.as_str())
    }

    /// Label of the edit that [`Self::redo`] would reapply.
    pub fn redo_label(&self) -> Option<&str> {
        self.future.last().map(|s| s.label.as_str())
    }

    /// `true` when there is something to undo.
    pub fn can_undo(&self) -> bool {
        !self.past.is_empty()
    }

    /// `true` when there is something to redo.
    pub fn can_redo(&self) -> bool {
        !self.future.is_empty()
    }

    /// Forgets all history, for example after opening a different project.
    pub fn clear(&mut self) {
        self.past.clear();
        self.future.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project_named(name: &str) -> Project {
        Project::new(name, 48_000.0)
    }

    #[test]
    fn undo_and_redo_walk_the_stack() {
        let mut history = History::default();
        let first = project_named("first");
        history.push("rename", &first);
        let second = project_named("second");

        let restored = history.undo(&second).unwrap();
        assert_eq!(restored.name, "first");
        assert!(!history.can_undo());
        assert!(history.can_redo());

        let redone = history.redo(&restored).unwrap();
        assert_eq!(redone.name, "second");
        assert!(history.can_undo());
    }

    #[test]
    fn a_new_edit_discards_the_redo_branch() {
        let mut history = History::default();
        history.push("a", &project_named("a"));
        let _ = history.undo(&project_named("b"));
        assert!(history.can_redo());

        history.push("c", &project_named("c"));
        assert!(!history.can_redo());
    }

    #[test]
    fn the_stack_is_bounded() {
        let mut history = History::new(3);
        for index in 0..10 {
            history.push(format!("edit {index}"), &project_named(&index.to_string()));
        }
        assert_eq!(history.past.len(), 3);
        // The oldest snapshots are dropped, so the remaining ones are the most recent.
        assert_eq!(history.undo_label(), Some("edit 9"));
    }

    #[test]
    fn undo_on_an_empty_history_is_a_no_op() {
        let mut history = History::default();
        assert!(history.undo(&project_named("x")).is_none());
        assert!(history.redo(&project_named("x")).is_none());
    }
}
