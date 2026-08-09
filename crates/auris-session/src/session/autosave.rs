//! Writing the document back over itself while somebody is working on it.
//!
//! # What this does and does not protect
//!
//! It writes the *real* file, not a recovery copy beside it. That is a trade with a name: the
//! document on disk is never more than [`AUTOSAVE_INTERVAL`] behind what is on screen, and in
//! exchange **"close without saving" stops being a way to undo an afternoon**. Undo still is, for
//! as long as the window is open.
//!
//! A recovery file instead would have kept both, at the price of a discovery flow — a dialog on
//! the next launch asking whether to adopt the copy, which is a thing people click through
//! without reading and then wonder where their work went. One file that is always current is
//! easier to reason about than two that disagree, so that is what this is, and the setting to
//! turn it off is one click away for anybody who wants the old bargain back.
//!
//! # What it will not do
//!
//! **It never invents a path.** A document that has never been saved has no folder, and choosing
//! one on somebody's behalf means their song is somewhere they did not put it. Saving that one is
//! still a question, and it is the frontend that asks it.
//!
//! **It never fires mid-gesture.** A drag is one undo step and, until it ends, a document caught
//! halfway through a change nobody has finished making.

use std::time::{Duration, Instant};

use super::Session;
use crate::error::SessionError;

/// How long after the last write the document is written again, if it has changed.
///
/// Thirty seconds. The file is JSON in the kilobytes and is written to a scratch file and renamed,
/// so the cost of one is not the reason for the number; the reason is that it bounds how much of a
/// take, a drag or an arrangement can be lost to a power cut to something nobody would call a
/// session's work.
pub const AUTOSAVE_INTERVAL: Duration = Duration::from_secs(30);

/// Everything the autosave policy looks at.
///
/// Gathered into one value so the decision can be a function with a test rather than a condition
/// buried in a poll loop, where the only way to check it would be to wait thirty seconds.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct AutosaveState {
    /// Whether the user has left the feature on.
    pub enabled: bool,
    /// Whether the document has somewhere to be written.
    pub has_path: bool,
    /// Whether it has changed since it was last written.
    pub dirty: bool,
    /// Whether a drag or another multi-step gesture is part way through.
    pub gesture_open: bool,
    /// How long since the document was last written, by any means.
    pub since_last_save: Duration,
}

/// Whether the document should be written back over itself now.
pub fn should_autosave(state: AutosaveState) -> bool {
    state.enabled
        && state.has_path
        && state.dirty
        && !state.gesture_open
        && state.since_last_save >= AUTOSAVE_INTERVAL
}

impl Session {
    /// Whether the document is being written back over itself as it changes.
    pub fn autosave_enabled(&self) -> bool {
        self.autosave
    }

    /// Turns autosaving on or off.
    ///
    /// Turning it on does not save anything immediately; the next tick that finds the document
    /// changed and the interval elapsed does.
    pub fn set_autosave(&mut self, enabled: bool) {
        self.autosave = enabled;
    }

    /// What the policy is looking at right now, for a frontend that wants to explain itself.
    pub fn autosave_state(&self) -> AutosaveState {
        AutosaveState {
            enabled: self.autosave,
            has_path: self.path.is_some(),
            dirty: self.dirty,
            gesture_open: self.transaction.is_some(),
            since_last_save: self.last_save.elapsed(),
        }
    }

    /// Writes the document back over itself if the policy says it is time.
    ///
    /// `None` means nothing was attempted, which is the answer almost every time it is asked.
    /// Call it from whatever the frontend already runs each frame — it is a handful of
    /// comparisons until the moment it is not.
    ///
    /// Deliberately not part of [`Session::poll`]. That is housekeeping and this writes to
    /// somebody's disk, and a method whose name promises the first should never quietly do the
    /// second.
    pub fn autosave(&mut self) -> Option<Result<(), SessionError>> {
        if !should_autosave(self.autosave_state()) {
            return None;
        }
        // Stamped whether or not the write succeeds: a disk that is refusing should be retried at
        // the same interval as everything else, not on every frame.
        self.last_save = Instant::now();
        Some(self.save_in_place())
    }

    /// Restarts the autosave clock. Called by every path that writes the document.
    pub(super) fn mark_saved(&mut self) {
        self.last_save = Instant::now();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document that would be saved: on, saved before, changed, idle, and overdue.
    fn ready() -> AutosaveState {
        AutosaveState {
            enabled: true,
            has_path: true,
            dirty: true,
            gesture_open: false,
            since_last_save: AUTOSAVE_INTERVAL,
        }
    }

    #[test]
    fn a_document_that_has_changed_is_written_once_the_interval_is_up() {
        assert!(should_autosave(ready()));
    }

    #[test]
    fn a_document_with_nowhere_to_go_is_never_written() {
        // The one that matters most: choosing a folder on somebody's behalf puts their song
        // somewhere they did not put it, and no interval makes that a good idea.
        assert!(!should_autosave(AutosaveState {
            has_path: false,
            ..ready()
        }));
    }

    #[test]
    fn nothing_is_written_part_way_through_a_gesture() {
        // A drag is one undo step and, until it ends, a document caught halfway through a change
        // nobody has finished making.
        assert!(!should_autosave(AutosaveState {
            gesture_open: true,
            ..ready()
        }));
    }

    #[test]
    fn an_unchanged_document_is_left_alone() {
        // Otherwise a project left open overnight would be rewritten twice a minute for no
        // reason, and its modification time would say it was worked on all night.
        assert!(!should_autosave(AutosaveState {
            dirty: false,
            ..ready()
        }));
    }

    #[test]
    fn the_interval_is_a_floor_rather_than_a_target() {
        let mut state = ready();
        state.since_last_save = AUTOSAVE_INTERVAL - Duration::from_millis(1);
        assert!(!should_autosave(state));

        state.since_last_save = AUTOSAVE_INTERVAL * 10;
        assert!(should_autosave(state), "an overdue save still happens");
    }

    #[test]
    fn switching_it_off_switches_it_off() {
        assert!(!should_autosave(AutosaveState {
            enabled: false,
            ..ready()
        }));
    }
}
