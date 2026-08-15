//! The key, the chords, the sections — and hearing any of them.
//!
//! None of these touch the engine. Harmony is a part of the document the render graph never
//! reads: the notes are already written, and changing the chord underneath them does not
//! change a sample. Composing *from* the harmony does rebuild the graph, but that is a
//! different command, and it is in `compose`.
//!
//! The structure — the labelled stretches a song is made of — is here for the same reason and
//! obeys the same rule: the notes already written do not move when the stretch around them is
//! renamed. What a label changes is what the composer will write *next* — a clip generated inside
//! a section draws its material from the label, so two stretches called サビ get recognisably the
//! same figures.
//!
//! Auditioning is here rather than beside the engine because harmony belongs to the timeline and
//! owns no instrument: hearing a chord means finding a track willing to sound it, which is
//! [`Session::audition_track`], and the four note commands underneath are the mechanism that does
//! the sounding. See [`crate::guide::harmony`] for why the voicing is deliberately not a part's.

use auris_core::TrackId;
use auris_core::harmony::Harmony;
use auris_core::project::{BEND_LIMIT, MODULATION_LIMIT};
use auris_core::theory::chart::{Chart, catalog};
use auris_core::theory::chord::Chord;
use auris_core::theory::key::Key as MusicalKey;
use auris_core::theory::numeral::Numeral;
use auris_core::theory::pitch::MIDDLE_C;
use auris_core::time::Ticks;
use auris_engine::EngineCommand;

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

impl Session {
    /// The key and the chords the song is written in, over the timeline.
    pub fn harmony(&self) -> &Harmony {
        &self.project.harmony
    }

    /// The grid a chord or a key change lands on at `at`: the beat, or the editing grid where that
    /// is coarser.
    ///
    /// Harmony is written coarser than notes are. A sixteenth-note editing grid is the right
    /// resolution for placing a hi-hat and the wrong one for placing a chord — nobody aiming at
    /// bar five means bar five and a sixteenth, and at a normal zoom the two are three pixels
    /// apart. The editing grid still wins when it is the coarser of the two, because somebody who
    /// set the grid to a bar asked for whole bars and should get them.
    ///
    /// Which beat, and so which grid, depends on where: an eighth is the beat in 7/8 and half of
    /// one in 3/4.
    pub fn harmony_grid_at(&self, at: Ticks) -> Ticks {
        self.project
            .signatures
            .signature_at(at)
            .ticks_per_beat()
            .max(self.project.grid)
    }

    /// Rounds a position onto [`Self::harmony_grid_at`]. What every harmony command writes through.
    ///
    /// Public because a frontend has to agree with it: a menu that offers "remove the chord here"
    /// only where one exists has to round the pointer the same way the command that writes them
    /// does, or the two disagree by a sixteenth and the item is never offered.
    ///
    /// Counted from the start of the stretch the meter is in force over rather than from tick
    /// zero. A bar line after a meter change need not be a multiple of the new beat — a 7/8 bar
    /// is 3360 ticks and a quarter note is 960 — so a grid counted from the origin would sit a
    /// fraction off every bar line for the rest of the song.
    pub fn snap_harmony(&self, at: Ticks) -> Ticks {
        let at = at.max_zero();
        let origin = self.project.signatures.change_at(at);
        origin + (at - origin).snap_nearest(self.harmony_grid_at(at))
    }

    /// Sets the key from `at` onwards.
    ///
    /// `at` snaps to the harmony grid, so a key change lands where a person aimed rather than
    /// where the pointer happened to be. Tick zero is the song's own key and is always there, so
    /// setting it there changes what the whole song is read in rather than adding a change to it.
    pub fn set_key(&mut self, at: Ticks, key: MusicalKey) {
        self.record(Edit::SetKey);
        let at = self.snap_harmony(at);
        self.project.harmony.keys.set_point(at, key);
    }

    /// Removes the key change in force at `at`, letting the key before it run through.
    ///
    /// *In force at*, not *starting at*: a key change is a boundary, and the stretch it governs
    /// runs to the next one. Removing the change that put the song in E flat means pointing
    /// anywhere in the E flat, which is the whole of what is on screen — rather than at the one
    /// grid position the change happens to sit on.
    ///
    /// The key at tick zero is not a change and cannot be removed: a song is always in some key.
    pub fn remove_key(&mut self, at: Ticks) {
        let at = self.project.harmony.keys.change_at(at.max_zero());
        if at == Ticks::ZERO {
            return;
        }
        self.record(Edit::SetKey);
        self.project.harmony.keys.remove_point(at);
    }

    /// Sets the chord sounding from `at` onwards, until the next change.
    pub fn set_chord(&mut self, at: Ticks, chord: Numeral) {
        self.record(Edit::SetChord);
        let at = self.snap_harmony(at);
        self.project.harmony.chords.set_point(at, Some(chord));
    }

    /// Removes the chord change in force at `at`, letting the chord before it run through.
    ///
    /// Found through [`ChordMap::change_at`](auris_core::harmony::ChordMap::change_at) rather
    /// than by rounding `at`, for the reason given on [`Self::remove_key`] and one more: a
    /// progression stamped three chords to a bar sits on thirds of a bar, which is not a position
    /// any editing grid can round to, so a rounded removal would silently miss every one of them.
    pub fn remove_chord(&mut self, at: Ticks) {
        let Some(at) = self.project.harmony.chords.change_at(at.max_zero()) else {
            return;
        };
        self.record(Edit::SetChord);
        self.project.harmony.chords.remove_point(at);
    }

    /// Moves the chord change in force at `from` to `to`, and says whether it moved one.
    ///
    /// `to` snaps to the harmony grid; `from` is resolved the way [`Self::remove_chord`] resolves
    /// its argument, so a drag can start anywhere inside the chord rather than on the one pixel
    /// it begins at. Dropping a chord onto another replaces that one.
    pub fn move_chord(&mut self, from: Ticks, to: Ticks) -> bool {
        let Some(from) = self.project.harmony.chords.change_at(from.max_zero()) else {
            return false;
        };
        let to = self.snap_harmony(to);
        if from == to {
            return false;
        }
        self.record(Edit::MoveChord);
        self.project.harmony.chords.move_point(from, to)
    }

    /// Empties the chords in `from..to`, leaving the key timeline alone.
    ///
    /// What sounded at `to` still sounds there: clearing the middle of a song does not silence
    /// the end of it.
    pub fn clear_harmony(&mut self, from: Ticks, to: Ticks) {
        self.record(Edit::ClearHarmony);
        let (from, to) = (self.snap_harmony(from), self.snap_harmony(to));
        self.project.harmony.clear(from, to);
    }

    /// Writes `chart` across `bars` bars from `from`, returning how many chords it wrote.
    ///
    /// The chart repeats or is truncated to fit. `from` snaps to the harmony grid, but the chords
    /// *inside* it do not: a chart divides each bar musically, and three chords in a bar of 4/4
    /// are three lots of 1280 ticks, which is not a grid position and must not be rounded to one.
    /// A stamp is a division of a bar; a drag is an edit on the grid.
    pub fn stamp_progression(&mut self, chart: &Chart, from: Ticks, bars: usize) -> usize {
        self.record(Edit::StampProgression);
        let from = self.snap_harmony(from);
        // The meter the chart begins in: a progression was written in bars of one meter, and a
        // change part way through the stamped range does not re-bar the chart behind it.
        let signature = self.project.signatures.signature_at(from);
        self.project.harmony.stamp(chart, from, bars, signature)
    }

    /// Names the section of the song beginning at the bar `at` falls in.
    ///
    /// Sections snap to bar lines rather than to the editing grid: 「サビはこの小節から」 is
    /// the thing being said, and a section starting mid-bar is not a thing a person means by
    /// pointing. `None` — or a name of nothing but whitespace — leaves the stretch from there
    /// deliberately unnamed, which is how a song's structure ends.
    pub fn set_section(&mut self, at: Ticks, label: Option<String>) {
        self.record(Edit::SetSection);
        let at = self.snap_section(at);
        self.project.sections.set_point(at, label);
    }

    /// Removes the section change in force at `at`, letting the one before it run through.
    ///
    /// *In force at*, not *starting at*, for the reason given on [`Self::remove_key`]: a
    /// section is a stretch, and pointing anywhere inside it is pointing at it.
    pub fn remove_section(&mut self, at: Ticks) {
        let Some(at) = self.project.sections.change_at(at.max_zero()) else {
            return;
        };
        self.record(Edit::SetSection);
        self.project.sections.remove_point(at);
    }

    /// Moves the section change in force at `from` to the start of the bar `to` falls in.
    pub fn move_section(&mut self, from: Ticks, to: Ticks) -> bool {
        let Some(from) = self.project.sections.change_at(from.max_zero()) else {
            return false;
        };
        let to = self.snap_section(to);
        if from == to {
            return false;
        }
        self.record(Edit::MoveSection);
        self.project.sections.move_point(from, to)
    }

    /// The start of the bar `at` falls in, which is the only place a section may begin.
    fn snap_section(&self, at: Ticks) -> Ticks {
        self.project.signatures.bar_floor(at)
    }

    /// Writes the catalogue progression called `name`, such as `axis` or `丸サ`.
    ///
    /// `bars` of zero means the chart's own length, which is what "put this progression here"
    /// usually means. A name nothing answers to is an error rather than a quiet no-op — there is
    /// no nearest right answer to a misspelling, and stamping nothing while reporting success is
    /// the one outcome nobody could debug.
    ///
    /// The chart is read against the key in force where it lands, so a major-mode progression
    /// dropped into a minor stretch names its chords from the relative key rather than having
    /// its degrees read literally: the same loop, centred where the music is.
    pub fn stamp_named_progression(
        &mut self,
        name: &str,
        from: Ticks,
        bars: usize,
    ) -> Result<usize, SessionError> {
        let chart =
            catalog(name).ok_or_else(|| SessionError::UnknownProgression(name.to_string()))?;
        let chart = chart.spelled_in(self.project.harmony.key_at(from.max_zero()));
        let bars = if bars == 0 { chart.bar_count() } else { bars };
        Ok(self.stamp_progression(&chart, from, bars))
    }

    /// Rounds a position onto the editing grid, and never before the start of the song.
    pub(super) fn snap(&self, at: Ticks) -> Ticks {
        at.max_zero().snap_nearest(self.project.grid)
    }

    /// Sounds a note on a track's instrument, outside the timeline.
    pub fn note_on(&mut self, track: TrackId, pitch: u8, velocity: f32) {
        if let Ok(index) = self.require_track(track) {
            self.send(EngineCommand::NoteOn {
                track: index,
                pitch: pitch.min(127),
                velocity: velocity.clamp(0.0, 1.0),
            });
        }
    }

    /// Releases an auditioned note.
    pub fn note_off(&mut self, track: TrackId, pitch: u8) {
        if let Ok(index) = self.require_track(track) {
            self.send(EngineCommand::NoteOff {
                track: index,
                pitch: pitch.min(127),
            });
        }
    }

    /// Sounds several notes at once, which is what it takes to hear a chord.
    pub fn notes_on(&mut self, track: TrackId, pitches: &[u8], velocity: f32) {
        for pitch in pitches {
            self.note_on(track, *pitch, velocity);
        }
    }

    /// Releases them.
    pub fn notes_off(&mut self, track: TrackId, pitches: &[u8]) {
        for pitch in pitches {
            self.note_off(track, *pitch);
        }
    }

    /// Bends everything a track's instrument is sounding, and everything it sounds next.
    ///
    /// Channel state rather than an event about a note, exactly as a wheel is: the instrument
    /// holds this until it is given another. Zero is what puts it back, and whoever bent it is
    /// the one who has to send that.
    ///
    /// Outside the timeline, like [`Self::note_on`] — a clip's own bend curve is a different
    /// thing, scheduled with the notes it belongs to.
    pub fn pitch_bend(&mut self, track: TrackId, semitones: f32) {
        if let Ok(index) = self.require_track(track) {
            self.send(EngineCommand::PitchBend {
                track: index,
                semitones: semitones.clamp(-BEND_LIMIT, BEND_LIMIT),
            });
        }
    }

    /// Moves a track's modulation wheel. Channel state, like the bend.
    pub fn modulation(&mut self, track: TrackId, amount: f32) {
        if let Ok(index) = self.require_track(track) {
            self.send(EngineCommand::Modulation {
                track: index,
                amount: amount.clamp(0.0, MODULATION_LIMIT),
            });
        }
    }

    /// The pitches to sound to hear the chord in force at `tick`.
    ///
    /// Empty when nothing is written there, which is the honest answer and not an error: the
    /// stretches between chords are part of a progression too.
    pub fn harmony_voicing(&self, tick: Ticks) -> Vec<u8> {
        self.project
            .harmony
            .chord_at(tick)
            .map(Self::voice_for_audition)
            .unwrap_or_default()
    }

    /// A chord laid out to be listened to rather than played by a part.
    ///
    /// The body sits around middle C, where a chord is easiest to identify, and the bass an
    /// octave and a half below it — far enough down to be heard as a bass rather than as the
    /// chord's own lowest note, which is what makes a slash chord audibly a slash chord.
    ///
    /// This is not what any part would play. A part has a register to keep, neighbours to stay out
    /// of the way of, and a previous chord to lead from; an audition has one job, which is to let
    /// somebody recognise the chord they just wrote down.
    pub fn voice_for_audition(chord: Chord) -> Vec<u8> {
        let mut pitches: Vec<i32> = chord.voiced_near(MIDDLE_C);
        pitches.push(chord.bass_class().midi(2));
        pitches.retain(|pitch| (0..=127).contains(pitch));
        pitches.sort_unstable();
        pitches.dedup();
        pitches.into_iter().map(|pitch| pitch as u8).collect()
    }

    /// A track that can sound an audition: `preferred` when it can, the first that can otherwise.
    ///
    /// Harmony belongs to the timeline rather than to any one track, so hearing it has to borrow
    /// somebody's instrument. Falling back matters more than it looks: writing the chords before
    /// the parts is the whole point of the lane, and at that moment the selected track may well be
    /// the audio track somebody imported a reference mix onto.
    pub fn audition_track(&self, preferred: Option<TrackId>) -> Option<TrackId> {
        let plays_notes = |id: TrackId| {
            self.project
                .track(id)
                .is_some_and(|track| track.kind.as_instrument().is_some())
        };
        preferred.filter(|id| plays_notes(*id)).or_else(|| {
            self.project
                .tracks
                .iter()
                .find(|track| track.kind.as_instrument().is_some())
                .map(|track| track.id)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, Scratch, numeral, session, with_a_progression};
    use auris_core::time::TimeSignature;
    use auris_core::{ClipPreset, ClipRecipe, Note};

    #[test]
    fn a_section_is_written_on_a_bar_line_and_found_from_anywhere_inside_it() {
        let bar = |n: i64| Ticks(3_840 * n);
        let mut session = session();
        session.set_section(Ticks(5), Some("イントロ".into()));
        session.set_section(bar(4) + Ticks(999), Some("サビ".into()));

        let points = session.project().sections.points();
        assert_eq!(points[0].tick, Ticks::ZERO, "snapped to its bar");
        assert_eq!(points[1].tick, bar(4));
        assert_eq!(
            session.project().sections.section_at(bar(6)),
            Some(("サビ", 1))
        );

        // Renaming is writing at the same bar; removing acts through the whole stretch.
        session.set_section(bar(4), Some("落ちサビ".into()));
        assert_eq!(
            session.project().sections.label_at(bar(5)),
            Some("落ちサビ")
        );
        session.remove_section(bar(7));
        assert_eq!(
            session.project().sections.label_at(bar(5)),
            Some("イントロ"),
            "the section before it runs through"
        );

        assert!(session.move_section(bar(2), bar(8) + Ticks(1)));
        assert_eq!(session.project().sections.points()[0].tick, bar(8));

        assert!(session.is_dirty());
        while session.undo().is_some() {}
        assert!(session.project().sections.is_empty());
    }

    #[test]
    fn a_generated_clip_reads_the_section_it_sits_in() {
        // The hint the structure exists for: labelling a stretch changes what the composer
        // writes there next. Same clip, same recipe, same harmony — a label appears under it,
        // and regenerating draws different material keyed by that label.
        let mut session = session();
        let track = session.add_default_instrument_track("Lead").expect("track");
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .expect("the catalogue knows axis");
        let recipe = ClipRecipe::new(ClipPreset::Lead, 7);
        let clip = session
            .generate_clip(track, Ticks::ZERO, Ticks::from_beats(16.0), recipe)
            .expect("generated");
        let unlabelled = session.midi_clip(clip).expect("clip").notes.clone();

        session.set_section(Ticks::ZERO, Some("サビ".into()));
        session.regenerate_clip(clip).expect("regenerated");
        let labelled = session.midi_clip(clip).expect("clip").notes.clone();
        assert_ne!(
            unlabelled, labelled,
            "the label should key the clip's material"
        );

        // And the same label writes the same take again: the hint is deterministic.
        session.regenerate_clip(clip).expect("regenerated again");
        assert_eq!(session.midi_clip(clip).expect("clip").notes, labelled);
    }

    #[test]
    fn harmony_snaps_to_the_beat_of_the_meter_it_is_written_in() {
        let mut session = session();
        // Seven eight: a bar is 3360 ticks, and the beat is an eighth rather than a quarter.
        session.set_signature_point(BAR, TimeSignature::new(7, 8));
        let seven_eight_bar = TimeSignature::new(7, 8).ticks_per_bar();

        // Counted from the change, not from tick zero. The second bar of 7/8 starts 3360 ticks
        // past a 3840-tick bar, which is not a multiple of anything the grid would offer — a
        // snap measured from the origin would sit a fraction off it.
        let second = BAR + seven_eight_bar;
        assert_eq!(session.snap_harmony(second + Ticks(20)), second);
        assert_eq!(
            session.harmony_grid_at(second),
            Ticks(auris_core::TICKS_PER_QUARTER / 2),
            "an eighth is the beat in seven eight"
        );
    }

    #[test]
    fn a_new_project_is_in_c_major_with_nothing_written_in_it() {
        let session = session();
        assert_eq!(session.harmony().key_at(Ticks::ZERO).to_text(), "C major");
        assert!(session.harmony().is_empty());
        assert_eq!(session.harmony().chord_at(Ticks::ZERO), None);
    }

    #[test]
    fn the_key_survives_undo_and_redo() {
        let mut session = self::tests::session();
        session.set_key(Ticks::ZERO, MusicalKey::parse("F# minor").unwrap());
        assert_eq!(session.harmony().key_at(BAR).to_text(), "F# minor");

        assert_eq!(session.undo(), Some(Edit::SetKey));
        assert_eq!(session.harmony().key_at(BAR).to_text(), "C major");
        assert_eq!(session.redo(), Some(Edit::SetKey));
        assert_eq!(session.harmony().key_at(BAR).to_text(), "F# minor");
    }

    #[test]
    fn the_key_at_the_start_of_the_song_cannot_be_removed() {
        let mut session = self::tests::session();
        session.set_key(Ticks::ZERO, MusicalKey::parse("D major").unwrap());
        session.forget_history();

        session.remove_key(Ticks::ZERO);
        assert_eq!(session.harmony().key_at(Ticks::ZERO).to_text(), "D major");
        assert!(
            !session.can_undo(),
            "a command that cannot do anything should not push an undo step"
        );
    }

    #[test]
    fn a_chord_lands_on_the_beat_rather_than_where_the_pointer_was() {
        let mut session = self::tests::session();
        // The editing grid is a sixteenth — 240 ticks — and harmony is written coarser than
        // that: a third of a beat past the bar line means the bar line.
        assert_eq!(session.project().grid, Ticks(240));
        assert_eq!(
            session.harmony_grid_at(Ticks::ZERO),
            Ticks(960),
            "one beat of 4/4"
        );

        session.set_chord(BAR + Ticks(300), numeral("V"));
        assert_eq!(session.harmony().chords.points()[0].tick, BAR);
        assert_eq!(session.harmony().numeral_at(BAR), Some(numeral("V")));

        // Two thirds of the way along is the next beat, not the next sixteenth.
        session.set_chord(BAR + Ticks(700), numeral("IV"));
        assert_eq!(
            session.harmony().chords.points()[1].tick,
            BAR + Ticks(960),
            "rounded up to beat two"
        );
    }

    #[test]
    fn a_grid_coarser_than_a_beat_is_what_a_chord_lands_on() {
        // Somebody who set the editing grid to a bar asked for whole bars, and harmony must not
        // quietly offer them something finer than they chose.
        let mut session = self::tests::session();
        session.set_grid(BAR);
        assert_eq!(session.harmony_grid_at(Ticks::ZERO), BAR);
        session.set_chord(BAR + Ticks(960), numeral("V"));
        assert_eq!(session.harmony().chords.points()[0].tick, BAR);
    }

    #[test]
    fn a_chord_is_removed_and_moved_by_pointing_anywhere_inside_it() {
        // A chord occupies everything up to the next change, and a stamp divides a bar musically
        // — three chords in a bar of 4/4 sit on thirds of it. Neither is reachable by rounding a
        // pointer position onto a grid, so both commands resolve through the change in force.
        let mut session = self::tests::session();
        session.set_chord(BAR, numeral("I"));
        session.set_chord(BAR * 4, numeral("V"));

        assert!(
            session.move_chord(BAR * 2 + Ticks(17), BAR * 3),
            "mid-chord"
        );
        assert_eq!(session.harmony().numeral_at(BAR * 3), Some(numeral("I")));
        assert_eq!(
            session.harmony().numeral_at(BAR * 2),
            None,
            "the chord left where it was, rather than being copied"
        );
        assert_eq!(session.undo(), Some(Edit::MoveChord));
        assert_eq!(session.harmony().numeral_at(BAR * 2), Some(numeral("I")));

        session.remove_chord(BAR * 2 + Ticks(17));
        assert!(session.harmony().numeral_at(BAR).is_none());
        assert_eq!(
            session.harmony().numeral_at(BAR * 4),
            Some(numeral("V")),
            "the one after it is untouched"
        );
    }

    #[test]
    fn nothing_to_move_or_remove_is_not_an_undo_step() {
        let mut session = self::tests::session();
        session.set_chord(BAR * 4, numeral("V"));
        session.forget_history();

        // Before the first chord there is nothing in force to act on.
        assert!(!session.move_chord(Ticks::ZERO, BAR));
        session.remove_chord(Ticks::ZERO);
        // And a move that rounds back onto where the chord already sits changes nothing.
        assert!(!session.move_chord(BAR * 4, BAR * 4 + Ticks(30)));
        assert!(!session.can_undo(), "none of those changed the document");
    }

    #[test]
    fn a_stamped_progression_is_one_undo_step_and_divides_its_bars_musically() {
        let mut session = self::tests::session();
        let written = session
            .stamp_named_progression("axis", Ticks::ZERO, 8)
            .unwrap();
        assert_eq!(written, 8, "four bars of the axis, laid down twice");
        assert_eq!(
            session.harmony().chord_at(Ticks::ZERO).unwrap().to_string(),
            "C"
        );
        assert_eq!(
            session.harmony().chord_at(BAR * 2).unwrap().to_string(),
            "Am"
        );

        // A three-chord bar is three lots of 1280, which is not a grid position — the stamp must
        // not have been snapped to one.
        session.forget_history();
        let chart = Chart::parse("| I V vi |").unwrap();
        session.stamp_progression(&chart, Ticks::ZERO, 1);
        let ticks: Vec<i64> = session
            .harmony()
            .chords
            .points()
            .iter()
            .take(3)
            .map(|point| point.tick.raw())
            .collect();
        assert_eq!(ticks, [0, 1280, 2560]);

        assert_eq!(session.undo(), Some(Edit::StampProgression));
        assert_eq!(
            session.harmony().chord_at(BAR * 2).unwrap().to_string(),
            "Am"
        );
    }

    #[test]
    fn a_progression_can_be_asked_for_by_the_name_a_japanese_musician_uses() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("丸サ", Ticks::ZERO, 0)
            .expect("the catalogue knows it under that name too");
        assert_eq!(
            session.harmony().chord_at(Ticks::ZERO).unwrap().to_string(),
            "Fmaj7",
            "bars of zero means the chart's own length"
        );
        assert_eq!(session.harmony().chords.points().len(), 4);
    }

    #[test]
    fn an_unknown_progression_is_an_error_and_writes_nothing() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .unwrap();
        session.forget_history();

        let before = session.project().harmony.clone();
        let error = session
            .stamp_named_progression("marusaa", Ticks::ZERO, 4)
            .unwrap_err();
        assert!(matches!(error, SessionError::UnknownProgression(name) if name == "marusaa"));
        assert_eq!(session.project().harmony, before, "nothing was written");
        assert!(!session.can_undo(), "and nothing was recorded either");
    }

    #[test]
    fn clearing_a_stretch_does_not_silence_what_comes_after_it() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 16)
            .unwrap();

        session.clear_harmony(BAR * 8, BAR * 12);
        assert!(
            session.harmony().chord_at(BAR * 7).is_some(),
            "before the gap"
        );
        assert!(session.harmony().chord_at(BAR * 9).is_none(), "inside it");
        assert_eq!(
            session.harmony().chord_at(BAR * 12).unwrap().to_string(),
            "C",
            "and the song picks up again on the far side"
        );
    }

    #[test]
    fn a_modulation_reharmonises_a_progression_without_rewriting_it() {
        let mut session = self::tests::session();
        session
            .stamp_named_progression("axis", Ticks::ZERO, 8)
            .unwrap();
        let before: Vec<Numeral> = session
            .harmony()
            .chords
            .points()
            .iter()
            .filter_map(|point| point.chord)
            .collect();

        session.set_key(BAR * 4, MusicalKey::parse("Eb major").unwrap());
        assert_eq!(
            session.harmony().chord_at(BAR * 3).unwrap().to_string(),
            "F"
        );
        assert_eq!(
            session.harmony().chord_at(BAR * 4).unwrap().to_string(),
            "Eb"
        );

        let after: Vec<Numeral> = session
            .harmony()
            .chords
            .points()
            .iter()
            .filter_map(|point| point.chord)
            .collect();
        assert_eq!(before, after, "not one chord was touched");
    }

    #[test]
    fn the_harmony_is_saved_and_comes_back() {
        let scratch = Scratch::new("harmony-round-trip");
        let mut session = self::tests::session();
        session.set_key(Ticks::ZERO, MusicalKey::parse("Bb minor").unwrap());
        session
            .stamp_named_progression("axis-minor", Ticks::ZERO, 4)
            .unwrap();
        let written = session.project().harmony.clone();

        let document = session
            .save_as(&scratch.join("Song.auris"))
            .unwrap()
            .document;
        let mut reopened = self::tests::session();
        reopened.open(&document).unwrap();
        assert_eq!(reopened.project().harmony, written);
        assert_eq!(reopened.harmony().key_at(Ticks::ZERO).to_text(), "Bb minor");
    }

    #[test]
    fn editing_the_harmony_leaves_the_notes_and_the_engine_alone() {
        let mut session = self::tests::session();
        let track = session.add_default_instrument_track("Keys").unwrap();
        let clip = session
            .add_midi_clip(track, "Riff", Ticks::ZERO, BAR)
            .expect("an empty clip");
        session
            .add_note(clip, Note::new(60, Ticks::ZERO, BAR))
            .unwrap();
        let notes_before = session.project().clone();
        session.forget_history();

        session
            .stamp_named_progression("axis", Ticks::ZERO, 4)
            .unwrap();
        assert_eq!(
            session.project().tracks,
            notes_before.tracks,
            "harmony is not a note and must not move one"
        );
    }

    #[test]
    fn the_chord_under_a_position_can_be_heard_and_the_silence_between_them_cannot() {
        let (session, _) = with_a_progression();

        // The axis progression in C major: I is C, and it sounds as one.
        let opening = session.harmony_voicing(Ticks::ZERO);
        assert!(!opening.is_empty(), "the first chord is silent");
        let chord = session.project().harmony.chord_at(Ticks::ZERO).unwrap();
        for pitch in &opening {
            assert!(
                chord.contains_midi(i32::from(*pitch)),
                "{pitch} is not in {chord}"
            );
        }

        // Nothing written is nothing sounded, rather than an error or a guess.
        let empty = self::tests::session();
        assert!(empty.harmony_voicing(Ticks::ZERO).is_empty());
    }

    #[test]
    fn an_audition_puts_the_bass_below_the_chord_and_the_chord_around_middle_c() {
        // A slash chord is the case that decides the layout: if the bass were voiced with the
        // rest, `C/E` and `C` would sound identical and the slash would be a silent decoration.
        let plain = Session::voice_for_audition(Chord::parse("C").unwrap());
        let slash = Session::voice_for_audition(Chord::parse("C/E").unwrap());
        assert_ne!(plain, slash);
        assert!(slash[0] < slash[1], "the bass is the lowest note");
        assert_eq!(
            i32::from(slash[0]) % 12,
            4,
            "the bass is the one that was named"
        );

        for chord in ["C", "F#m", "Bbmaj7", "G7", "D9"] {
            let pitches = Session::voice_for_audition(Chord::parse(chord).unwrap());
            let body = &pitches[1..];
            assert!(pitches[0] < 48, "{chord}: the bass is not a bass");
            assert!(
                body.iter().all(|pitch| (48..=96).contains(pitch)),
                "{chord} left the register a chord is recognised in: {pitches:?}"
            );
            assert!(
                pitches.windows(2).all(|pair| pair[0] < pair[1]),
                "{chord} sounded a pitch twice: {pitches:?}"
            );
        }
    }

    #[test]
    fn an_audition_borrows_an_instrument_when_the_selection_cannot_play_one() {
        // The case this exists for: chords are written before parts are, so the selected track at
        // that moment may be an audio track — or there may be no selection at all.
        let mut session = self::tests::session();
        let audio = session.add_audio_track("Reference");
        assert_eq!(
            session.audition_track(Some(audio)),
            None,
            "nothing can play"
        );

        let instrument = session.add_default_instrument_track("Piano").unwrap();
        assert_eq!(session.audition_track(None), Some(instrument));
        assert_eq!(session.audition_track(Some(audio)), Some(instrument));
        assert_eq!(
            session.audition_track(Some(instrument)),
            Some(instrument),
            "a track that can play keeps the audition"
        );
    }
}
