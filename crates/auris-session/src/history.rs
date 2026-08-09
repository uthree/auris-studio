//! Undo/redo over whole-project snapshots.
//!
//! A [`Project`] holds ids, numbers and note lists — never audio samples — so a clone costs a
//! few kilobytes even for a large arrangement. Snapshotting the whole document is therefore
//! affordable, and it removes a whole class of bugs that command-based undo suffers from, where
//! one edit path forgets to record its inverse.

use auris_core::Project;
use auris_core::project::{ClipCurve, ClipId};
use auris_core::time::Ticks;

use crate::param::ParamTarget;

/// What one undo step reverses.
///
/// An enum rather than a display string: the session must not decide what words a frontend
/// shows, and a localised frontend cannot translate a `String` that arrived from here. Adding a
/// variant makes every frontend's `match` fail to compile, which is the reminder we want.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Edit {
    /// Cycle playback was turned on or off.
    ToggleLoop,
    /// The cycle region was moved.
    SetLoopRegion,
    /// The tempo of one stretch of the timeline was turned.
    ///
    /// The position of the change being turned travels along, because repeated-edit coalescing
    /// compares whole `Edit` values — see [`Edit::AdjustParameter`]. Without it, nudging the
    /// tempo here and then seeking elsewhere and nudging there folded into a single step.
    ChangeTempo(Ticks),
    /// A tempo change was written somewhere on the timeline.
    SetTempoPoint,
    /// A tempo change was removed, letting the tempo before it run through.
    RemoveTempoPoint,
    /// The time signature of one stretch of the timeline was changed.
    ///
    /// Carries the position of the change being turned, for the reason [`Edit::ChangeTempo`]
    /// does: the readout's wheel coalesces, and two stretches must not fold into one step.
    ChangeSignature(Ticks),
    /// A signature change was written somewhere on the timeline.
    SetSignaturePoint,
    /// A signature change was removed, letting the meter before it run through.
    RemoveSignaturePoint,
    /// An instrument track was added.
    AddInstrumentTrack,
    /// An audio track was added.
    AddAudioTrack,
    /// A bus was added.
    AddBusTrack,
    /// A track was deleted.
    DeleteTrack,
    /// A track was copied.
    DuplicateTrack,
    /// A track was moved up or down the list.
    MoveTrack,
    /// A track was renamed.
    RenameTrack,
    /// A track was given a different colour.
    SetTrackColor,
    /// A track was muted or unmuted.
    MuteTrack,
    /// A track was soloed or unsoloed.
    SoloTrack,
    /// A track's output was pointed somewhere else.
    SetTrackOutput,
    /// A send was added to a track.
    AddSend,
    /// A send was removed from a track.
    ///
    /// A send's *level* has no variant of its own: it is a mixer control like a fader, so turning
    /// one records [`Edit::AdjustParameter`] over [`ParamTarget::Send`] and coalesces the same way.
    RemoveSend,
    /// A send was moved before or after the fader.
    SetSendPreFader,
    /// A track's instrument was replaced.
    ChangeInstrument,
    /// A key, a progression and a set of parts were written around a melody.
    Accompany,
    /// A clip was created.
    AddClip,
    /// A clip was deleted.
    DeleteClip,
    /// Clips were taken to the clipboard.
    ///
    /// The same removal as [`Edit::DeleteClip`] and a variant of its own all the same: what Undo
    /// is called decides whether somebody believes the material is gone or believes it is waiting
    /// to be pasted, and those are different things to believe.
    CutClips,
    /// Clips were laid down from the clipboard.
    PasteClips,
    /// A clip was copied.
    DuplicateClip,
    /// A clip was divided in two.
    SplitClip,
    /// A clip was renamed.
    RenameClip,
    /// A clip was muted or unmuted.
    MuteClip,
    /// A clip was moved along the timeline.
    MoveClip,
    /// A clip's length changed.
    ResizeClip,
    /// A clip was set to repeat, or stopped repeating.
    ///
    /// Its own variant rather than [`Edit::ResizeClip`] because the two edges do different things
    /// and undo has to say which one is coming back: a resize changes what the clip *is*, and this
    /// changes only how many times it is said.
    LoopClip,
    /// An audio clip's own gain changed.
    SetClipGain,
    /// An audio clip's fade-in or fade-out changed.
    SetClipFade,
    /// A note was added.
    AddNote,
    /// Notes were deleted.
    DeleteNotes,
    /// Notes were taken to the clipboard. See [`Edit::CutClips`] for why this is not a delete.
    CutNotes,
    /// Notes were laid down from the clipboard.
    PasteNotes,
    /// Notes were copied.
    DuplicateNotes,
    /// Notes were shifted in pitch.
    TransposeNotes,
    /// Notes were struck harder or softer.
    SetNoteVelocity,
    /// Notes were moved.
    MoveNotes,
    /// A note's length changed.
    ResizeNote,
    /// Notes were snapped onto a division of the beat.
    QuantizeNotes,
    /// An effect was added to a chain.
    AddEffect,
    /// An effect was removed from a chain.
    RemoveEffect,
    /// An effect was bypassed or re-enabled.
    BypassEffect,
    /// A chain was reordered.
    ReorderEffects,
    /// A parameter value changed.
    ///
    /// Which parameter travels along, because repeated-edit coalescing compares whole `Edit`
    /// values: without the target, a cutoff notch and a fader notch made within the window
    /// folded into a single step, and undo took both back at once.
    AdjustParameter(ParamTarget),
    /// A point was written on a parameter's automation lane, or moved along it.
    ///
    /// Which parameter travels along for the reason [`Edit::AdjustParameter`]'s does: a point
    /// dragged on one lane and then a point dragged on another must not fold into one step.
    WriteAutomation(ParamTarget),
    /// A point was taken off a lane.
    EraseAutomation,
    /// A point was written on a clip's pitch bend, or moved along it.
    ///
    /// Which clip travels along for the reason [`Edit::WriteAutomation`]'s parameter does: a bend
    /// drawn on one clip and then on another must not fold into a single undo step.
    WriteBend(ClipId),
    /// A point was taken off a clip's bend, or the whole curve was.
    EraseBend,
    /// A point was written on a clip's modulation, or moved along it.
    WriteModulation(ClipId),
    /// A point was taken off a clip's modulation, or the whole curve was.
    EraseModulation,
    /// A whole lane was removed, giving the parameter its stored value back.
    ClearAutomation,
    /// An audio file was imported onto a track.
    ImportAudio,
    /// A take was recorded onto an audio track.
    RecordTake,
    /// A SoundFont was imported into the project.
    ImportSoundFont,
    /// A track was pointed at one of a SoundFont's sounds.
    ChoosePreset,
    /// The key changed somewhere on the timeline.
    SetKey,
    /// A chord changed somewhere on the timeline.
    SetChord,
    /// A chord moved along the timeline.
    MoveChord,
    /// A stretch of the timeline lost its chords.
    ClearHarmony,
    /// A section label changed somewhere on the timeline — written, renamed or removed.
    SetSection,
    /// A section boundary moved along the timeline.
    MoveSection,
    /// A named progression was written across a range of bars.
    StampProgression,
    /// A clip was written from the harmony under it.
    GenerateClip,
    /// A generated clip stopped being generated.
    FreezeClip,
    /// The document was replaced by a composed piece.
    Compose,
}

impl Edit {
    /// The step that writing on `which` records.
    ///
    /// The two curves share every command, and this is where they stop sharing: an undo menu
    /// saying "the pitch bend" over a wheel movement would be a menu that lies about the thing it
    /// is offering to take back.
    pub fn write_curve(which: ClipCurve, clip: ClipId) -> Edit {
        match which {
            ClipCurve::Bend => Edit::WriteBend(clip),
            ClipCurve::Modulation => Edit::WriteModulation(clip),
        }
    }

    /// The step that erasing from `which` records.
    pub fn erase_curve(which: ClipCurve) -> Edit {
        match which {
            ClipCurve::Bend => Edit::EraseBend,
            ClipCurve::Modulation => Edit::EraseModulation,
        }
    }
}

/// A bounded undo/redo stack of project snapshots.
pub struct History {
    past: Vec<Snapshot>,
    future: Vec<Snapshot>,
    limit: usize,
}

struct Snapshot {
    edit: Edit,
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

    /// Records the state *before* `edit`.
    ///
    /// Call this immediately before mutating the project. Recording a new edit discards the
    /// redo stack, which is what every editor does after diverging from an undone branch.
    pub fn push(&mut self, edit: Edit, project: &Project) {
        self.future.clear();
        self.past.push(Snapshot {
            edit,
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
            edit: snapshot.edit,
            project: current.clone(),
        });
        Some(snapshot.project)
    }

    /// Steps forward, returning the project state to restore.
    pub fn redo(&mut self, current: &Project) -> Option<Project> {
        let snapshot = self.future.pop()?;
        self.past.push(Snapshot {
            edit: snapshot.edit,
            project: current.clone(),
        });
        Some(snapshot.project)
    }

    /// The edit that [`Self::undo`] would reverse.
    pub fn undo_edit(&self) -> Option<Edit> {
        self.past.last().map(|s| s.edit)
    }

    /// The edit that [`Self::redo`] would reapply.
    pub fn redo_edit(&self) -> Option<Edit> {
        self.future.last().map(|s| s.edit)
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
        history.push(Edit::RenameTrack, &first);
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
        history.push(Edit::AddClip, &project_named("a"));
        let _ = history.undo(&project_named("b"));
        assert!(history.can_redo());

        history.push(Edit::AddNote, &project_named("c"));
        assert!(!history.can_redo());
    }

    #[test]
    fn the_stack_is_bounded() {
        let mut history = History::new(3);
        for index in 0..10 {
            let edit = if index == 9 {
                Edit::MoveNotes
            } else {
                Edit::MoveClip
            };
            history.push(edit, &project_named(&index.to_string()));
        }
        assert_eq!(history.past.len(), 3);
        // The oldest snapshots are dropped, so the remaining ones are the most recent.
        assert_eq!(history.undo_edit(), Some(Edit::MoveNotes));
        assert_eq!(history.undo(&project_named("x")).unwrap().name, "9");
    }

    #[test]
    fn undo_on_an_empty_history_is_a_no_op() {
        let mut history = History::default();
        assert!(history.undo(&project_named("x")).is_none());
        assert!(history.redo(&project_named("x")).is_none());
    }
}
