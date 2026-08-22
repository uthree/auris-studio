//! The clock, and the two maps that decide what a position on it means.
//!
//! Playing, stopping, seeking and the loop region belong together because they are one thing seen
//! from this side: a clock the audio thread owns and that this side asks questions of and sends
//! commands to. The tempo trio sits with them because a tempo is part of the same clock — notes
//! are scheduled in frames, so moving the tempo moves every one of them, and each of these
//! re-flattens the render graph and republishes the loop region.
//!
//! The meter is the tempo trio again, and deliberately shaped the same way, but none of these
//! touch the engine. A meter is notation: the notes are written in ticks, the tempo map turns
//! ticks into samples, and neither asks how many beats are in a bar. Editing this moves the bar
//! lines and not one sample.
//!
//! The click is the one exception, and it is a narrow one. A metronome accents bar lines, so the
//! engine holds a copy of the signature map for exactly that — nothing else down there reads it.
//! Rather than give the meter commands a rebuild that would be pure waste on every project where
//! the click is off, a meter change is *remembered* ([`Session::meter_is_stale`]) and republished
//! the moment something is listening: now, if the click is already on, and otherwise not until it
//! is switched on.
//!
//! Where the tempo commands take any position, these land on bar lines — see
//! [`SignatureMap`](auris_core::time::SignatureMap) for why a change that did not would leave
//! the bars after it uncountable.

use auris_core::time::{Seconds, Ticks, TimeSignature};
use auris_engine::EngineCommand;

use crate::history::Edit;

use super::Session;

impl Session {
    /// Starts playback.
    pub fn play(&mut self) {
        self.send(EngineCommand::Play);
    }

    /// Stops playback, leaving the playhead where it is.
    pub fn stop(&mut self) {
        self.send(EngineCommand::Stop);
    }

    /// Starts or stops playback.
    pub fn toggle_play(&mut self) {
        if self.is_playing() {
            self.stop();
        } else {
            self.play();
        }
    }

    /// `true` when the transport is rolling, read from the audio thread itself.
    pub fn is_playing(&self) -> bool {
        self.engine.is_playing()
    }

    /// Moves the playhead, clamping to the timeline start.
    pub fn seek(&mut self, tick: Ticks) {
        let frames = self
            .project
            .tempo_map
            .ticks_to_samples(tick.max_zero(), self.engine.sample_rate())
            .raw();
        self.send(EngineCommand::Seek { frames });
    }

    /// Where the playhead is, in ticks.
    pub fn playhead(&self) -> Ticks {
        self.project
            .tempo_map
            .seconds_to_ticks(Seconds(self.engine.playhead_seconds()))
    }

    /// Silences every voice.
    pub fn panic(&mut self) {
        self.send(EngineCommand::Panic);
    }

    /// Turns looping on or off, seeding a two-bar region when there is none.
    ///
    /// Deliberately not recorded. Cycling is how a user listens, not something they write: a
    /// loop-and-listen pass would otherwise fill the undo stack with toggles and push the edits
    /// the pass was checking off the end of it. Dragging the region *is* recorded — that is
    /// aimed at a place in the song rather than at the transport.
    pub fn set_loop_enabled(&mut self, enabled: bool) {
        self.dirty = true;
        self.project.loop_enabled = enabled;
        if enabled && self.project.loop_region.is_none() {
            // Bars one and two, asked for as bars rather than as twice a bar length: with a meter
            // change in the second bar those are not the same span, and the one a person means by
            // "the first two bars" is this one.
            self.project.loop_region = Some((Ticks::ZERO, self.project.signatures.bar_start(3)));
        }
        self.publish_loop();
    }

    /// Sets the loop region. A region that is not positive disables the loop.
    pub fn set_loop_region(&mut self, start: Ticks, end: Ticks) {
        self.record(Edit::SetLoopRegion);
        let (start, end) = if end < start {
            (end, start)
        } else {
            (start, end)
        };
        self.project.loop_region = Some((start.max_zero(), end.max_zero()));
        self.publish_loop();
    }

    /// Sends the loop region to the audio thread.
    ///
    /// The region is stored in ticks and the transport holds frames, so this has to run again
    /// after anything that moves the mapping between them — a document swap or a tempo change.
    pub(super) fn publish_loop(&self) {
        let (start, end) = self
            .project
            .loop_region
            .unwrap_or((Ticks::ZERO, Ticks::ZERO));
        let rate = self.engine.sample_rate();
        self.send(EngineCommand::SetLoop {
            enabled: self.project.loop_enabled && end > start,
            start: self.project.tempo_map.ticks_to_samples(start, rate).raw(),
            end: self.project.tempo_map.ticks_to_samples(end, rate).raw(),
        });
    }

    /// Whether a click is heard on every beat while the transport rolls.
    pub fn metronome(&self) -> bool {
        self.project.metronome
    }

    /// Turns the click on or off.
    ///
    /// Deliberately not recorded, for the reason [`Self::set_loop_enabled`] is not: playing along
    /// to a click is how a user listens rather than something they write, and a practice pass
    /// would otherwise fill the undo stack with toggles. It is a stored document field all the
    /// same, so it has to reach the file — unmarked, a click switched on and the document closed
    /// went without the unsaved prompt and was quietly lost.
    pub fn set_metronome(&mut self, enabled: bool) {
        if self.project.metronome == enabled {
            return;
        }
        self.project.metronome = enabled;
        self.dirty = true;
        // A switch is a switch and costs nothing; what may be out of date is the map the accents
        // are counted against. Paid for here, once, rather than by every meter change made while
        // nobody was listening.
        if enabled && self.meter_is_stale {
            self.invalidate_graph();
        } else {
            self.send(EngineCommand::SetMetronome(enabled));
        }
    }

    /// Starts or stops the click.
    pub fn toggle_metronome(&mut self) {
        self.set_metronome(!self.project.metronome);
    }

    /// How many bars are counted in front of a take, or zero when none are.
    pub fn count_in_bars(&self) -> u32 {
        self.project.count_in_bars
    }

    /// Sets how many bars are counted in front of a take.
    ///
    /// Clamped to [`Session::MAX_COUNT_IN_BARS`], and not recorded — it is preparation, the same
    /// as arming a track or switching the click on, and a pass of try-it-and-undo would fill the
    /// stack with settings and push the take off the end of it. Marked dirty all the same, so it
    /// reaches the file rather than being quietly lost at the next close.
    pub fn set_count_in_bars(&mut self, bars: u32) {
        let bars = bars.min(Self::MAX_COUNT_IN_BARS);
        if self.project.count_in_bars == bars {
            return;
        }
        self.project.count_in_bars = bars;
        self.dirty = true;
    }

    /// Frames of count-in left before the transport starts moving, zero when none is running.
    ///
    /// For a frontend that has to show why Record is lit and nothing is happening yet. It counts
    /// down in the *engine's* frames, which is what a readout wants: the count is a wait in the
    /// room, not a distance along the timeline.
    pub fn count_in_frames(&self) -> u64 {
        self.engine.count_in_frames()
    }

    /// `true` while a count-in is being played.
    pub fn counting_in(&self) -> bool {
        self.count_in_frames() > 0
    }

    /// Beats of the count-in still to be played, counting the one sounding now.
    ///
    /// Zero when no count is running. What a readout wants: a count-in is heard in beats, and
    /// "three" beside a Record button that has not started moving says what is happening in the
    /// one unit somebody is already counting in their head.
    pub fn count_in_beats_left(&self) -> u32 {
        let frames = self.count_in_frames();
        let beat = self.counting.map_or(0, |count| count.beat_frames);
        match (frames, beat) {
            (0, _) | (_, 0) => 0,
            (frames, beat) => frames.div_ceil(beat) as u32,
        }
    }

    /// Remembers that the bar lines have moved, and republishes them if anything is listening.
    fn meter_changed(&mut self) {
        self.meter_is_stale = true;
        if self.project.metronome {
            self.invalidate_graph();
        }
    }

    /// Sets the project tempo at the start of the timeline.
    ///
    /// The whole-song knob: it turns the stretch that begins at tick zero and leaves any tempo
    /// changes written further along where they are. A transport readout parked mid-song wants
    /// [`Self::set_tempo_at`] with the playhead instead.
    pub fn set_bpm(&mut self, bpm: f64) {
        self.set_tempo_at(Ticks::ZERO, bpm);
    }

    /// Replaces the tempo of the stretch `at` falls in.
    ///
    /// A wheel over the readout arrives as a stream of small deltas, and re-flattening the
    /// graph is the expensive half of this. A value that has not moved is not a change — and
    /// it is the *clamped* value that decides, or holding the wheel past 999 would keep
    /// recording steps that change nothing. The map is a handful of points; probing a clone
    /// is cheaper than the rebuild it saves.
    ///
    /// The recorded edit carries the position of the change being turned, so nudging the tempo
    /// of one stretch and then another stays two undo steps however quickly the hand moves.
    pub fn set_tempo_at(&mut self, at: Ticks, bpm: f64) {
        let at = self.project.tempo_map.change_at(at.max_zero());
        let mut probe = self.project.tempo_map.clone();
        probe.set_point(at, bpm);
        if probe == self.project.tempo_map {
            return;
        }
        self.record_repeating(Edit::ChangeTempo(at));
        self.project.tempo_map = probe;
        // Notes are scheduled in frames, so the graph has to be re-flattened, and the loop's
        // frame positions move with it.
        self.invalidate_graph();
        self.publish_loop();
    }

    /// Sets the tempo from `at` onwards, writing a change on the beat `at` rounds to.
    ///
    /// `at` snaps the way the harmony does — see [`Self::snap_harmony`] — because a tempo
    /// change is aimed at a place in the song, not at the sixteenth the pointer happened to
    /// cross. Writing at tick zero turns the song's opening tempo rather than adding to it,
    /// exactly as [`Self::set_key`] treats the anchor.
    pub fn set_tempo_point(&mut self, at: Ticks, bpm: f64) {
        let at = self.snap_harmony(at);
        let mut probe = self.project.tempo_map.clone();
        probe.set_point(at, bpm);
        if probe == self.project.tempo_map {
            return;
        }
        self.record(Edit::SetTempoPoint);
        self.project.tempo_map = probe;
        self.invalidate_graph();
        self.publish_loop();
    }

    /// Removes the tempo change in force at `at`, letting the tempo before it run through.
    ///
    /// *In force at*, not *starting at*, for the reason given on [`Self::remove_key`]. The
    /// anchor at tick zero is not a change and cannot be removed: a song always has a tempo.
    pub fn remove_tempo_point(&mut self, at: Ticks) {
        let at = self.project.tempo_map.change_at(at.max_zero());
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::RemoveTempoPoint);
        self.project.tempo_map.remove_point(at);
        self.invalidate_graph();
        self.publish_loop();
    }

    /// The signature in force at `at`.
    pub fn signature_at(&self, at: Ticks) -> TimeSignature {
        self.project.signatures.signature_at(at)
    }

    /// Replaces the signature of the stretch `at` falls in.
    ///
    /// The counterpart of [`Self::set_tempo_at`], and coalescing for the same reason: the wheel
    /// over the transport readout arrives as a stream of steps, and a meter that has not moved is
    /// not a change. The recorded edit carries the position of the change being turned, so
    /// nudging one stretch and then another stays two undo steps.
    pub fn set_signature_at(&mut self, at: Ticks, signature: TimeSignature) {
        let at = self.project.signatures.change_at(at.max_zero());
        let mut probe = self.project.signatures.clone();
        probe.set_point(at, signature);
        if probe == self.project.signatures {
            return;
        }
        self.record_repeating(Edit::ChangeSignature(at));
        self.project.signatures = probe;
        self.meter_changed();
    }

    /// Sets the signature from `at` onwards, writing a change on the bar `at` rounds to.
    ///
    /// The ruler's counterpart to [`Self::set_signature_at`]. Writing at tick zero turns the
    /// song's opening meter rather than adding a change to it, exactly as [`Self::set_key`] and
    /// [`Self::set_tempo_point`] treat the anchor.
    pub fn set_signature_point(&mut self, at: Ticks, signature: TimeSignature) {
        let mut probe = self.project.signatures.clone();
        probe.set_point(at.max_zero(), signature);
        if probe == self.project.signatures {
            return;
        }
        self.record(Edit::SetSignaturePoint);
        self.project.signatures = probe;
        self.meter_changed();
    }

    /// Removes the signature change in force at `at`, letting the meter before it run through.
    ///
    /// *In force at*, not *starting at*, for the reason given on [`Self::remove_key`]. The anchor
    /// at tick zero is not a change and cannot be removed: a song is always in some meter.
    pub fn remove_signature_point(&mut self, at: Ticks) {
        let at = self.project.signatures.change_at(at.max_zero());
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::RemoveSignaturePoint);
        self.project.signatures.remove_point(at);
        self.meter_changed();
    }

    /// Sets the editing grid.
    pub fn set_grid(&mut self, grid: Ticks) {
        let grid = Ticks(grid.raw().max(1));
        if self.project.grid == grid {
            return;
        }
        self.project.grid = grid;
        // Not recorded — cycling the grid is a view-adjacent tweak nobody wants on the undo
        // stack — but it is a stored document field and has to reach the file: unmarked, a
        // grid-only change closed without the unsaved prompt and was quietly lost.
        self.dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, session, undo_depth};
    use auris_core::Note;

    #[test]
    fn a_tempo_that_has_not_moved_is_not_an_edit() {
        let mut session = session();
        session.forget_history();
        session.set_bpm(session.project().bpm());
        assert!(!session.can_undo());
        // And neither is one the clamp refuses, however long the wheel is held past the end.
        session.set_bpm(10_000.0);
        let steps = undo_depth(&mut session);
        assert_eq!(steps, 1, "the first push through the ceiling did move it");
        session.set_bpm(20_000.0);
        assert_eq!(undo_depth(&mut session), steps, "and the next one did not");
        // A written change that changes nothing is not an edit either.
        session.set_tempo_point(Ticks::ZERO, session.project().bpm());
        assert_eq!(undo_depth(&mut session), steps);
    }

    #[test]
    fn a_tempo_change_lands_on_the_beat_and_stays_undoable() {
        let mut session = session();
        session.forget_history();

        // A pointer aims a little past the second beat; the change lands on the beat itself.
        session.set_tempo_point(Ticks(970), 90.0);
        let points = session.project().tempo_map.points();
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].tick, Ticks(960));
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(959)), 120.0);
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(960)), 90.0);
        // The initial tempo is untouched: the change is a change, not the project knob.
        assert_eq!(session.project().bpm(), 120.0);

        assert_eq!(session.undo(), Some(Edit::SetTempoPoint));
        assert_eq!(session.project().tempo_map.points().len(), 1);
    }

    #[test]
    fn editing_the_tempo_edits_the_stretch_it_is_aimed_at() {
        let mut session = session();
        session.set_tempo_point(Ticks(3_840), 90.0);
        session.forget_history();

        // Aimed mid-stretch, the edit turns the change governing that stretch rather than
        // writing a new one.
        session.set_tempo_at(Ticks(5_000), 96.0);
        assert_eq!(session.project().tempo_map.points().len(), 2);
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(3_840)), 96.0);
        assert_eq!(
            session.project().bpm(),
            120.0,
            "the opening stretch kept its own"
        );

        // Turning the opening stretch straight afterwards is its own undo step: the recorded
        // edits carry different positions, so they can never coalesce however fast they come.
        session.set_tempo_at(Ticks::ZERO, 110.0);
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks::ZERO)));
        assert_eq!(session.undo(), Some(Edit::ChangeTempo(Ticks(3_840))));
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(3_840)), 90.0);
    }

    #[test]
    fn removing_a_tempo_change_is_aimed_from_anywhere_inside_it() {
        let mut session = session();
        session.set_tempo_point(Ticks(3_840), 90.0);
        session.forget_history();

        // The anchor is not a change: pointing inside the opening stretch removes nothing.
        session.remove_tempo_point(Ticks(100));
        assert_eq!(session.project().tempo_map.points().len(), 2);
        assert!(
            !session.can_undo(),
            "refusing to remove the anchor is not an edit"
        );

        // Pointing far past the change still removes the change in force there.
        session.remove_tempo_point(Ticks(50_000));
        assert_eq!(session.project().tempo_map.points().len(), 1);
        assert_eq!(session.project().tempo_map.bpm_at(Ticks(3_840)), 120.0);
        assert_eq!(session.undo(), Some(Edit::RemoveTempoPoint));
        assert_eq!(session.project().tempo_map.points().len(), 2);
    }

    #[test]
    fn a_signature_change_lands_on_a_bar_and_comes_back_off_it() {
        let mut session = session();
        session.forget_history();
        let three_four = TimeSignature::new(3, 4);

        // A pointer lands mid-bar; the change lands on the bar line it was aimed at.
        session.set_signature_point(Ticks(BAR.raw() * 2 + 400), three_four);
        let points = session.project().signatures.points();
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].tick, BAR * 2);
        assert_eq!(session.signature_at(BAR * 2), three_four);
        assert_eq!(
            session.signature_at(BAR * 2 - Ticks(1)),
            TimeSignature::default(),
            "the bars before it are what they were"
        );
        // And the bar numbering follows: bar 3 is where the 3/4 starts, bar 4 three beats later.
        assert_eq!(session.project().signatures.bar_of(BAR * 2), 3);
        assert_eq!(
            session.project().signatures.bar_start(4),
            BAR * 2 + three_four.ticks_per_bar()
        );

        assert_eq!(session.undo(), Some(Edit::SetSignaturePoint));
        assert!(session.project().signatures.is_constant());
    }

    #[test]
    fn editing_the_signature_edits_the_stretch_it_is_aimed_at() {
        let mut session = session();
        session.set_signature_point(BAR * 4, TimeSignature::new(3, 4));
        session.forget_history();

        // Aimed mid-stretch, the edit turns the change governing that stretch rather than
        // writing a new one.
        session.set_signature_at(BAR * 6, TimeSignature::new(7, 8));
        assert_eq!(session.project().signatures.points().len(), 2);
        assert_eq!(session.signature_at(BAR * 4), TimeSignature::new(7, 8));
        assert_eq!(
            session.project().signatures.initial(),
            TimeSignature::default(),
            "the opening stretch kept its own"
        );

        // Turning the opening stretch straight afterwards is its own undo step: the recorded
        // edits carry different positions, so they can never coalesce however fast they come.
        session.set_signature_at(Ticks::ZERO, TimeSignature::new(5, 4));
        assert_eq!(session.undo(), Some(Edit::ChangeSignature(Ticks::ZERO)));
        assert_eq!(session.undo(), Some(Edit::ChangeSignature(BAR * 4)));
        assert_eq!(session.signature_at(BAR * 4), TimeSignature::new(3, 4));
    }

    #[test]
    fn removing_a_signature_change_is_aimed_from_anywhere_inside_it() {
        let mut session = session();
        session.set_signature_point(BAR * 4, TimeSignature::new(3, 4));
        session.forget_history();

        // The anchor is not a change: pointing inside the opening stretch removes nothing.
        session.remove_signature_point(Ticks(100));
        assert_eq!(session.project().signatures.points().len(), 2);
        assert!(
            !session.can_undo(),
            "refusing to remove the anchor is not an edit"
        );

        // Pointing far past the change still removes the change in force there.
        session.remove_signature_point(Ticks(500_000));
        assert!(session.project().signatures.is_constant());
        assert_eq!(session.undo(), Some(Edit::RemoveSignaturePoint));
        assert_eq!(session.project().signatures.points().len(), 2);
    }

    #[test]
    fn a_meter_change_moves_the_bar_lines_and_not_one_note() {
        // The whole reason this is not on the audio path. A note is a tick position; the tempo
        // map turns ticks into samples; neither asks how many beats are in a bar.
        let mut session = session();
        let track = session
            .add_default_instrument_track("Lead")
            .expect("the registry has an instrument");
        let clip = session
            .add_midi_clip(track, "Part", Ticks::ZERO, BAR * 4)
            .expect("an instrument track takes a midi clip");
        session
            .add_note(clip, Note::new(60, BAR * 2, BAR))
            .expect("the note fits the clip");
        let before = session.project().midi_clip(clip).unwrap().1.notes.clone();
        let seconds = session.project().duration_seconds();

        session.set_signature_point(BAR, TimeSignature::new(7, 8));

        assert_eq!(
            session.project().midi_clip(clip).unwrap().1.notes,
            before,
            "a note moved when the meter changed"
        );
        assert_eq!(
            session.project().duration_seconds(),
            seconds,
            "the song got longer or shorter when the meter changed"
        );
    }

    #[test]
    fn the_click_is_listening_rather_than_writing_and_still_reaches_the_file() {
        let mut session = session();
        session.add_default_instrument_track("Lead").unwrap();
        assert!(!session.metronome());

        session.toggle_metronome();
        assert!(session.metronome());
        assert!(
            session.is_dirty(),
            "a click switched on has to survive being closed and reopened"
        );
        // A practice pass is a run of toggles, and none of them is an edit — or the notes the
        // pass was checking would be pushed off the end of the undo stack by it.
        for _ in 0..8 {
            session.toggle_metronome();
        }
        assert_eq!(session.undo(), Some(Edit::AddInstrumentTrack));

        // Setting it to what it already is does nothing at all.
        session.forget_history();
        session.set_metronome(session.metronome());
        assert!(!session.is_dirty());
    }

    #[test]
    fn the_bar_lines_reach_the_click_however_the_two_are_ordered() {
        // The engine holds a copy of the signature map so the click knows which beat to accent,
        // and a meter change is otherwise none of its business. So the change is remembered while
        // nothing is listening and republished the moment something is — whichever way round the
        // two happen.
        let mut session = session();
        assert!(!session.meter_is_stale);

        // Meter first, click afterwards: the stale map is paid for when the click is switched on.
        session.set_signature_point(BAR * 4, TimeSignature::new(3, 4));
        assert!(session.meter_is_stale, "the engine's copy is out of date");
        session.set_metronome(true);
        assert!(
            !session.meter_is_stale,
            "the click was switched on against bar lines that had moved"
        );

        // Click first, meter afterwards: the change republishes at once.
        session.set_signature_point(BAR * 8, TimeSignature::new(5, 4));
        assert!(!session.meter_is_stale);

        // And with the click off again, a meter change costs the engine nothing.
        session.set_metronome(false);
        session.remove_signature_point(BAR * 8);
        assert!(session.meter_is_stale);
    }

    #[test]
    fn cycling_is_listening_and_does_not_land_on_the_undo_stack() {
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").unwrap();
        session
            .add_midi_clip(track, "Riff", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();

        for _ in 0..4 {
            session.set_loop_enabled(true);
            session.set_loop_enabled(false);
        }
        assert_eq!(
            session.undo(),
            Some(Edit::AddClip),
            "the edits are still there"
        );
    }
}
