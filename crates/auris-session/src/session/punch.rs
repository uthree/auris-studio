//! Punch recording: a take that keeps only the bars it was asked for.
//!
//! The case this exists for is one bad bar in an otherwise good take. Without it, fixing bar nine
//! means recording over the whole passage and hoping the rest comes out as well as it did the
//! first time. With it, the player rolls from bar five, hears everything they already have, plays
//! through bar nine and rolls out again — and only bar nine changes.
//!
//! # What punch does and does not do
//!
//! It is **not** an automatic Record. Pressing Record is still how a take begins, because a
//! transport that started writing to disk on its own — because the playhead happened to cross a
//! region somebody set an hour ago — is a transport nobody would leave rolling. What punch decides
//! is what a take that *is* running keeps.
//!
//! So: press Record wherever you like, play, and the take is trimmed to the punch region on its
//! way to becoming a clip. The transport rolls out of the take by itself at the punch-out, which
//! is the part nobody wants to do by hand while playing.
//!
//! # Why the take is trimmed rather than gated
//!
//! Gating the capture callback — feeding the pool only inside the region — was the other design,
//! and it is worse in both directions. It is *less* accurate, because the callback can only
//! compare against the playhead once per block and the trim is arithmetic on a frame count. And it
//! is more fragile: the input runs on its own clock, so the callback's idea of where the playhead
//! is arrives one block stale.
//!
//! The file therefore holds the whole take and the clip holds the punch. That is a few seconds of
//! disk nobody asked for, and it buys a sample-accurate edge and a take that can be recovered by
//! hand from the project folder when the punch was set to the wrong bar.
//!
//! # Clearing the way
//!
//! A punched take **removes what it lands on**, on its own track, as part of the same undo step.
//! Recording without punch cannot do this — nothing knows where a take will end until it ends —
//! but a punch says up front which bars are being replaced, and leaving the old audio underneath
//! would play both at once. That is not a subtle mix problem; it is the bar you were fixing
//! playing behind the fix.
//!
//! What is cleared is exactly what the new clip covers, which is not always the whole region: a
//! player who rolled in two beats late replaces two beats less. Clearing the region regardless
//! would delete audio that nothing was recorded over.

use auris_core::{Ticks, TrackId};

use super::Session;
use crate::history::Edit;

impl Session {
    /// The punch region, whether or not it is switched on.
    pub fn punch_region(&self) -> Option<(Ticks, Ticks)> {
        self.project.punch_region
    }

    /// Whether a take is trimmed to the punch region.
    pub fn punch_enabled(&self) -> bool {
        self.project.punch_enabled
    }

    /// Turns punch on or off, seeding a two-bar region when there is none.
    ///
    /// Deliberately not an undo step, for the reason [`set_loop_enabled`](Session::set_loop_enabled)
    /// is not: it is how somebody prepares to play rather than something they wrote, and a pass of
    /// try-punch-undo would fill the stack with switches and push the take off the end of it.
    /// Dragging the region *is* recorded — that is aimed at a place in the song.
    pub fn set_punch_enabled(&mut self, enabled: bool) {
        self.dirty = true;
        self.project.punch_enabled = enabled;
        if enabled && self.project.punch_region.is_none() {
            // Whatever the loop is set to, when there is one: somebody who has just looped four
            // bars to find the bad one is about to punch those bars. Otherwise bars one and two,
            // asked for as bars because a meter change makes those two spans different lengths.
            self.project.punch_region = Some(
                self.project
                    .loop_region
                    .unwrap_or((Ticks::ZERO, self.project.signatures.bar_start(3))),
            );
        }
    }

    /// Sets the punch region.
    pub fn set_punch_region(&mut self, start: Ticks, end: Ticks) {
        self.record(Edit::SetPunchRegion);
        let (start, end) = match end < start {
            true => (end, start),
            false => (start, end),
        };
        self.project.punch_region = Some((start.max_zero(), end.max_zero()));
    }

    /// The punch region in frames at `rate`, when punch is on and the region is a positive span.
    ///
    /// The rate is asked for rather than assumed because the two callers count in different ones:
    /// the transport runs at the *device's* rate and a recorded buffer is resampled to the
    /// *project's*. A punch measured in the wrong one lands in the wrong bar on any machine whose
    /// hardware disagrees with the document, which is an ordinary machine.
    ///
    /// `None` means every frame of a take is kept — punch off, or a region dragged to nothing.
    pub(super) fn punch_frames_at(&self, rate: f64) -> Option<(u64, u64)> {
        let (start, end) = self.project.punch_region?;
        if !self.project.punch_enabled || end <= start {
            return None;
        }
        let rate = rate.max(1.0);
        Some((
            self.project.tempo_map.ticks_to_samples(start, rate).raw(),
            self.project.tempo_map.ticks_to_samples(end, rate).raw(),
        ))
    }

    /// Ends a take that has left the punch region, and turns it into a clip.
    ///
    /// Called from a frontend's frame loop, like [`autosave`](Session::autosave), and for the same
    /// shape of reason: this is not something a command returns, it is something that becomes true
    /// while nobody is calling anything. `None` when there is nothing to do, which is almost always.
    ///
    /// Rolling out by hand is the one part of punching that cannot be done while playing — both
    /// hands are busy, which is why the take was being punched rather than re-recorded.
    ///
    /// **Left**, not *passed*. Under a cycle the playhead never gets past the punch-out: it wraps
    /// before it, so a take that waited for `playhead >= out` would run until somebody stopped it
    /// — which is the one thing this exists to save them doing. Leaving needs having been in, so
    /// the take remembers that it was.
    pub fn finish_punch(&mut self) -> Option<Result<super::RecordingReport, crate::SessionError>> {
        let (in_at, out) = self.punch_frames_at(self.engine.sample_rate())?;
        // The engine's own playhead rather than a tick reading, because that is the clock the
        // take's start was stamped against and the two must be compared in the same units.
        let playhead = self.engine.playhead_frames();
        let take = self.take.as_mut()?;
        if (in_at..out).contains(&playhead) {
            take.entered_punch = true;
            return None;
        }
        match take.entered_punch {
            true => Some(self.stop_recording()),
            // Not there yet: Record was pressed before the punch-in, which is the ordinary way to
            // punch — the player wants to hear the bars before their own.
            false => None,
        }
    }

    /// Clears the punch region on `track`, so a punched take is not heard over what it replaces.
    ///
    /// Every clip the region touches is split at whichever edges fall inside it and the middle is
    /// removed, so a clip that merely overlaps keeps the part that does. Recorded as part of the
    /// take's own undo step by the caller having opened one first.
    pub(super) fn clear_punch_range(&mut self, track: TrackId, start: Ticks, end: Ticks) {
        if end <= start {
            return;
        }
        let inside: Vec<(auris_core::ClipId, Ticks, Ticks)> = self
            .project
            .track(track)
            .and_then(|track| track.kind.as_audio())
            .map(|audio| {
                audio
                    .clips
                    .iter()
                    .map(|clip| {
                        let length = self.audio_clip_length_ticks(clip);
                        (clip.id, clip.start, clip.start + length)
                    })
                    .filter(|(_, from, to)| *to > start && *from < end)
                    .collect()
            })
            .unwrap_or_default();

        for (clip, from, to) in inside {
            // Split at the far edge first: splitting at the near one would renumber nothing but it
            // *would* leave the second half addressed by a new id, and the far edge is inside it.
            let tail = match to > end && from < end {
                true => self.split_clip(clip, end).ok(),
                false => None,
            };
            let head = match from < start && to > start {
                true => self.split_clip(clip, start).ok(),
                false => None,
            };
            // What is left in the middle: the second half of a split at the punch-in, or the
            // original clip when it began inside the region.
            let middle = head.unwrap_or(clip);
            let _ = self.remove_clip(middle);
            let _ = tail;
        }
    }
}

/// Which frames of a take survive the punch, as an offset into it and a length.
///
/// A free function over four numbers because it is the whole of the trim and every one of its
/// cases is a take somebody is waiting for: a take that began before the punch-in, one that was
/// stopped before the punch-out, and one that never overlapped the region at all — which is a
/// player who rolled past their own punch and gets told so rather than getting an empty clip.
///
/// `None` means nothing survives. `Some((0, frames))` means the whole take does, which is what
/// punch being off produces.
pub(super) fn punch_window(
    punch: Option<(u64, u64)>,
    start: u64,
    frames: u64,
) -> Option<(u64, u64)> {
    let Some((in_at, out_at)) = punch else {
        return Some((0, frames));
    };
    let end = start + frames;
    let first = in_at.max(start);
    let last = out_at.min(end);
    match last > first {
        true => Some((first - start, last - first)),
        false => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Session, SessionOptions};

    fn session() -> Session {
        Session::new(SessionOptions::headless()).expect("a headless session")
    }

    #[test]
    fn punch_is_off_and_has_no_region_until_it_is_asked_for() {
        let session = session();
        assert!(!session.punch_enabled());
        assert_eq!(session.punch_region(), None);
    }

    #[test]
    fn switching_punch_on_borrows_the_loop_when_there_is_one() {
        // Somebody who has just looped four bars to find the bad one is about to punch those bars.
        // Seeding from bar one instead would be a region they have to drag every single time.
        let mut looped = session();
        looped.set_loop_region(Ticks::from_beats(8.0), Ticks::from_beats(16.0));
        looped.set_punch_enabled(true);
        assert_eq!(
            looped.punch_region(),
            Some((Ticks::from_beats(8.0), Ticks::from_beats(16.0)))
        );

        // And two bars when there is no loop to borrow, asked for as bars: with a meter change in
        // the second bar, twice a bar length is not the span anybody means.
        let mut unlooped = session();
        unlooped.set_punch_enabled(true);
        let (from, to) = unlooped.punch_region().expect("a seeded region");
        assert_eq!(from, Ticks::ZERO);
        assert_eq!(to, unlooped.project().signatures.bar_start(3));
    }

    #[test]
    fn a_region_dragged_backwards_is_still_a_region() {
        let mut session = session();
        session.set_punch_region(Ticks::from_beats(16.0), Ticks::from_beats(8.0));
        assert_eq!(
            session.punch_region(),
            Some((Ticks::from_beats(8.0), Ticks::from_beats(16.0)))
        );
    }

    #[test]
    fn switching_punch_on_is_not_an_undo_step_but_dragging_it_is() {
        // Try-punch-undo is a pass somebody makes several times before the take they keep, and a
        // stack full of switches is a stack the take has been pushed off the end of.
        let mut session = session();
        let before = session.history.undo_edit();
        session.set_punch_enabled(true);
        session.set_punch_enabled(false);
        assert_eq!(session.history.undo_edit(), before);

        session.set_punch_region(Ticks::ZERO, Ticks::from_beats(4.0));
        assert_eq!(session.history.undo_edit(), Some(Edit::SetPunchRegion));
    }

    #[test]
    fn punch_off_keeps_every_frame_of_the_take() {
        assert_eq!(punch_window(None, 0, 1_000), Some((0, 1_000)));
        assert_eq!(punch_window(None, 48_000, 1_000), Some((0, 1_000)));
    }

    #[test]
    fn a_take_is_trimmed_to_whichever_edges_fall_inside_it() {
        // Rolling in early and out late is the ordinary way to punch: the player wants to hear the
        // bars before theirs, and to keep playing past the edit rather than stop dead on it.
        assert_eq!(punch_window(Some((100, 200)), 0, 1_000), Some((100, 100)));
        // Record pressed after the punch-in: everything from where the take actually begins.
        assert_eq!(punch_window(Some((100, 200)), 150, 1_000), Some((0, 50)));
        // Stopped before the punch-out: everything up to where the take actually ends.
        assert_eq!(punch_window(Some((100, 200)), 0, 150), Some((100, 50)));
        // Wholly inside the region, so nothing is trimmed at all.
        assert_eq!(punch_window(Some((100, 200)), 120, 40), Some((0, 40)));
    }

    #[test]
    fn a_take_that_never_reached_the_punch_is_kept_from_becoming_an_empty_clip() {
        // The player rolled past their own punch, or stopped before reaching it. A zero-length
        // clip at the punch-in would be a thing to find and delete rather than an answer.
        assert_eq!(
            punch_window(Some((100, 200)), 0, 100),
            None,
            "stopped short"
        );
        assert_eq!(
            punch_window(Some((100, 200)), 200, 100),
            None,
            "rolled past"
        );
        assert_eq!(punch_window(Some((100, 200)), 300, 100), None);
        // Touching at a single frame is not an overlap either.
        assert_eq!(punch_window(Some((100, 200)), 100, 0), None);
    }

    #[test]
    fn clearing_the_punch_range_leaves_the_parts_of_a_clip_that_are_outside_it() {
        // The whole point of punching: bar nine changes and the bars around it do not. A clip
        // spanning the region has to come back as two, not as none.
        let mut session = session();
        let track = session.add_audio_track("Vocals");
        let source = session.project.add_audio_source(
            "take",
            auris_core::AssetPath::inside("Audio/take.wav"),
            48_000 * 16,
            48_000.0,
            1,
        );
        session
            .project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("a clip");

        session.clear_punch_range(track, Ticks::from_beats(4.0), Ticks::from_beats(8.0));
        let clips = session
            .project()
            .track(track)
            .and_then(|track| track.kind.as_audio())
            .map(|audio| audio.clips.clone())
            .unwrap_or_default();
        assert_eq!(clips.len(), 2, "the clip should survive on both sides");
        assert!(clips.iter().any(|clip| clip.start == Ticks::ZERO));
        assert!(
            clips
                .iter()
                .any(|clip| clip.start == Ticks::from_beats(8.0)),
            "the part after the punch-out is missing"
        );
    }

    #[test]
    fn clearing_inside_a_transaction_is_one_undo_step() {
        // What `stop_recording` relies on, and what it did not used to open. Clearing goes through
        // `split_clip` and `remove_clip`, and outside a transaction each of those records a step of
        // its own — so punching over one clip that spanned the region pushed three entries on top
        // of the take's, and the single Undo the user meant for "the recording" landed them between
        // the two splits, holding fragments with new ids and no take.
        let mut session = session();
        let track = session.add_audio_track("Vocals");
        let source = session.project.add_audio_source(
            "take",
            auris_core::AssetPath::inside("Audio/take.wav"),
            48_000 * 16,
            48_000.0,
            1,
        );
        let original = session
            .project
            .add_audio_clip(track, source, Ticks::ZERO)
            .expect("a clip");
        session.forget_history();

        session.begin_transaction(Edit::RecordTake);
        session.clear_punch_range(track, Ticks::from_beats(4.0), Ticks::from_beats(8.0));
        assert!(session.end_transaction(), "the clearing changed something");

        assert_eq!(session.undo(), Some(Edit::RecordTake));
        assert!(
            !session.can_undo(),
            "the splits and the removal were one step"
        );
        let clips = session
            .project()
            .track(track)
            .and_then(|track| track.kind.as_audio())
            .map(|audio| audio.clips.clone())
            .unwrap_or_default();
        assert_eq!(clips.len(), 1, "one Undo puts the clip back whole");
        assert_eq!(
            clips[0].id, original,
            "and with the id it had, not a fragment's"
        );
    }

    #[test]
    fn clearing_removes_a_clip_that_lies_wholly_inside_the_range() {
        let mut session = session();
        let track = session.add_audio_track("Vocals");
        let source = session.project.add_audio_source(
            "take",
            auris_core::AssetPath::inside("Audio/take.wav"),
            48_000,
            48_000.0,
            1,
        );
        session
            .project
            .add_audio_clip(track, source, Ticks::from_beats(5.0))
            .expect("a clip");

        session.clear_punch_range(track, Ticks::from_beats(4.0), Ticks::from_beats(16.0));
        let count = session
            .project()
            .track(track)
            .and_then(|track| track.kind.as_audio())
            .map(|audio| audio.clips.len())
            .unwrap_or_default();
        assert_eq!(count, 0);
    }
}
