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
use std::{hash::DefaultHasher, hash::Hasher};

use super::Session;

fn file_fingerprint(path: &std::path::Path) -> Option<u64> {
    let bytes = std::fs::read(path).ok()?;
    let mut hasher = DefaultHasher::new();
    hasher.write(&bytes);
    Some(hasher.finish())
}
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
    /// Whether the file on disk has been changed by another writer since this session last
    /// read or wrote it.
    pub overwritten: bool,
    /// How long since the document was last written, by any means.
    pub since_last_save: Duration,
}

/// Whether the document should be written back over itself now.
pub fn should_autosave(state: AutosaveState) -> bool {
    state.enabled
        && state.has_path
        && state.dirty
        && !state.gesture_open
        // Another writer's version is on disk. Automatically writing over it would silently
        // destroy work this window never saw. In-place saves refuse too; accepting the disk
        // changes or saving to another project resolves the disagreement.
        && !state.overwritten
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
            overwritten: self.externally_modified(),
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

    /// Restarts the autosave clock. Called by every path that writes the document — and by
    /// `open`, which is the other way this session and the file come to agree.
    ///
    /// The disk stamp is taken here because this is that agreement's one funnel: whatever the
    /// file's modification time is at this moment is *ours*, and a different one later means
    /// another writer — see [`Session::externally_modified`].
    pub(super) fn mark_saved(&mut self) {
        self.last_save = Instant::now();
        self.saved_project = self.project.clone();
        self.disk_stamp = self
            .path
            .as_deref()
            .and_then(|path| std::fs::metadata(path).ok())
            .and_then(|meta| meta.modified().ok());
        self.disk_fingerprint = self.path.as_deref().and_then(file_fingerprint);
    }

    /// Whether the file on disk is no longer the one this session last read or wrote.
    ///
    /// `true` means another writer has been at it — the MCP door, a sync service, anything —
    /// and what this session would save is based on a version that is no longer there.
    /// A file that cannot be examined (deleted, unreadable) answers `false`: that is a
    /// different problem, and the next save will say so in its own words.
    pub fn externally_modified(&self) -> bool {
        let (Some(path), Some(stamp)) = (self.path.as_deref(), self.disk_stamp) else {
            return false;
        };
        match std::fs::metadata(path).and_then(|meta| meta.modified()) {
            Ok(now) => {
                now != stamp
                    || self
                        .disk_fingerprint
                        .zip(file_fingerprint(path))
                        .is_some_and(|(saved, current)| saved != current)
            }
            Err(_) => false,
        }
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
            overwritten: false,
            since_last_save: AUTOSAVE_INTERVAL,
        }
    }

    #[test]
    fn another_writers_version_is_never_silently_overwritten() {
        // The MCP door, a sync service — whoever wrote it, autosave must not destroy it.
        // Saving over it is a decision, and ⌘S is where decisions are made.
        assert!(!should_autosave(AutosaveState {
            overwritten: true,
            ..ready()
        }));
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

    /// The stamp agrees after every read and write of the file, and disagrees — visibly —
    /// the moment another writer touches it.
    #[test]
    fn another_writer_is_noticed_and_a_save_of_our_own_is_not() {
        let root = std::env::temp_dir().join(format!("auris-session-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        session.save_as(&root.join("Watched.auris")).unwrap();
        assert!(
            !session.externally_modified(),
            "the file just written is our own"
        );

        // Another writer, played by a bumped modification time — how it looks from here,
        // however the bytes changed. Set explicitly rather than written and slept for,
        // because a fast filesystem gives two writes in one timestamp.
        let document = session.path().unwrap().to_path_buf();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&document)
            .unwrap();
        file.set_modified(std::time::SystemTime::now() + Duration::from_secs(2))
            .unwrap();
        drop(file);
        assert!(session.externally_modified(), "the other writer shows");
        assert!(
            session.autosave_state().overwritten,
            "and the autosave policy sees it"
        );

        // Saving by hand is the deliberate act that takes the file back.
        session.save_in_place().unwrap();
        assert!(!session.externally_modified(), "ours again");

        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn another_writer_is_noticed_when_the_timestamp_does_not_move() {
        let root =
            std::env::temp_dir().join(format!("auris-session-same-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut session =
            crate::Session::new(crate::SessionOptions::headless()).expect("a headless session");
        session.save_as(&root.join("Watched.auris")).unwrap();
        let document = session.path().unwrap().to_path_buf();
        let stamp = session.disk_stamp.unwrap();
        let mut bytes = std::fs::read(&document).unwrap();
        let index = bytes.iter().position(|byte| *byte == b' ').unwrap_or(0);
        bytes[index] = if bytes[index] == b' ' { b'\t' } else { b' ' };
        std::fs::write(&document, bytes).unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&document)
            .unwrap();
        file.set_modified(stamp).unwrap();
        drop(file);

        assert!(session.externally_modified());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
