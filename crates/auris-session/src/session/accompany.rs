//! Writing parts around a melody somebody played.
//!
//! The composer's other direction, seen from this layer. [`Session::compose`] takes a
//! specification and produces a whole document; this takes a clip that is already in one and puts
//! a band behind it.
//!
//! # Why it is not a smaller version of composing
//!
//! Because it must not replace anything. The melody is the point — it is the thing the user wrote
//! — so this adds tracks beside it, writes a key and a progression under it, and touches not one
//! note of the clip it was aimed at. That is also why it is a transaction rather than a document
//! swap: a dozen edits that have to be one step, over a document that goes on being the document.
//!
//! # What it gets wrong, and why that is the design
//!
//! The chords come from [`auris_compose::analysis`], which reads one voice and can be fooled by
//! one — see that module's header for the two ways. Everything it produces is therefore *editable
//! and re-runnable*: the key and the chords go into the harmony lane where they can be seen and
//! corrected, and every part it writes carries a [`ClipRecipe`], so correcting a chord and
//! pressing regenerate rewrites the parts around the correction. It is a first draft somebody
//! argues with, and it is built to be argued with.

use auris_compose::analysis::{self, Reading};
use auris_core::theory::key::Key as MusicalKey;
use auris_core::time::Ticks;
use auris_core::{ClipId, ClipPreset, ClipRecipe, Note, PresetRef, TrackId};

use crate::error::SessionError;
use crate::history::Edit;

use super::Session;

/// The parts written when a caller does not say which it wants.
///
/// A rhythm section and nothing else. A pad and an arpeggio are the two that most often turn an
/// accompaniment into an arrangement — which is a thing to add on purpose rather than to be handed
/// — and drums, bass and a comp are what "put some backing behind this" means to nearly everybody.
pub const DEFAULT_PARTS: [ClipPreset; 3] =
    [ClipPreset::Bass, ClipPreset::Chords, ClipPreset::Drums];

/// What accompanying a melody produced.
#[derive(Clone, Debug, PartialEq)]
pub struct AccompanyReport {
    /// The key the melody was read in.
    ///
    /// Worth reporting even when it is right: it is the one guess everything else rests on, so a
    /// person who does not recognise it has found the reason the rest sounds wrong.
    pub key: MusicalKey,
    /// How many chords were written under the melody.
    pub chords: usize,
    /// How many bars the melody covers.
    pub bars: usize,
    /// The tracks that were added, in the order they were written.
    pub parts: Vec<TrackId>,
    /// How many notes those parts hold between them.
    pub notes: usize,
    /// `true` when the parts are playing the built-in oscillators because the General MIDI font
    /// is not installed.
    ///
    /// Not a failure — the piece plays — and worth saying, because the difference between this
    /// and a working accompaniment is most of what somebody would hear.
    pub substituted: bool,
}

impl Session {
    /// Writes a key, a progression and a set of backing parts around a melody clip.
    ///
    /// One undo step for the lot. The clip itself is not touched.
    ///
    /// `seed` is the first part's; the rest take the numbers after it, so two parts written
    /// together are two different takes rather than the same figure on two instruments — and
    /// running it again from the same seed gives the same band back.
    ///
    /// The detected key is written at the melody's start, which turns the song's key when the
    /// melody begins at bar one and writes a change when it does not — the ordinary behaviour of
    /// [`Session::set_key`]. It is written rather than merely used, so that it is *visible*: a
    /// harmony this got wrong is a harmony somebody can see and retype, and the parts regenerate
    /// around the correction.
    pub fn accompany(
        &mut self,
        clip: ClipId,
        parts: &[ClipPreset],
        seed: u64,
    ) -> Result<AccompanyReport, SessionError> {
        let Some((_, midi)) = self.project.midi_clip(clip) else {
            return Err(SessionError::UnknownClip(clip.0));
        };
        if midi.notes.is_empty() {
            return Err(SessionError::NothingToAccompany(clip.0));
        }
        let start = midi.start;
        // Timeline positions, not clip-relative ones. The bar lines the harmony is read against
        // belong to the song, and a melody in a clip starting mid-bar is a melody whose downbeats
        // are not at its own tick zero.
        let melody: Vec<Note> = midi
            .notes
            .iter()
            .map(|note| Note {
                start: start + note.start,
                ..note.clone()
            })
            .collect();

        let reading = analysis::read_melody(&melody, start, self.signature_at(start));
        let bar_ticks = Ticks(self.signature_at(start).ticks_per_bar().raw().max(1));
        let length = bar_ticks * reading.bars as i64;

        // Everything from here is one gesture. Validated first, so a refusal above costs neither
        // an undo step nor the redo branch.
        self.begin_transaction(Edit::Accompany);
        let report = self.write_accompaniment(&reading, start, length, parts, seed);
        if !self.end_transaction() {
            // Nothing changed — which can only mean the reading wrote no chords and no part
            // produced a note. Leaving no step behind is the honest answer.
            return Ok(report);
        }
        Ok(report)
    }

    /// The body of [`Self::accompany`], inside its transaction.
    fn write_accompaniment(
        &mut self,
        reading: &Reading,
        start: Ticks,
        length: Ticks,
        parts: &[ClipPreset],
        seed: u64,
    ) -> AccompanyReport {
        self.set_key(start, reading.key);
        let chords = self.stamp_progression(&reading.chart, start, reading.bars);

        // Once for the whole set rather than once per part, and only if a part is actually going
        // to ask for a sound out of it.
        let font = (!parts.is_empty())
            .then(|| self.adopt_general_midi_here())
            .flatten();

        let mut report = AccompanyReport {
            key: reading.key,
            chords,
            bars: reading.bars,
            parts: Vec::with_capacity(parts.len()),
            notes: 0,
            substituted: font.is_none() && !parts.is_empty(),
        };

        for (index, preset) in parts.iter().enumerate() {
            let Ok(track) = self.add_default_instrument_track(part_name(*preset)) else {
                // No instrument in the registry at all, which no build has — and if one did, the
                // chords are still written and worth keeping.
                continue;
            };
            if let Some(font) = font {
                let sound = analysis::sound_for(*preset);
                let _ = self.set_track_preset(
                    track,
                    PresetRef {
                        font,
                        bank: i32::from(sound.bank),
                        patch: i32::from(sound.patch),
                    },
                );
            }
            let recipe = ClipRecipe::new(*preset, seed.wrapping_add(index as u64));
            match self.generate_clip(track, start, length, recipe) {
                Ok(clip) => {
                    report.notes += self.midi_clip(clip).map_or(0, |midi| midi.notes.len());
                    report.parts.push(track);
                }
                // The track is kept even so: an empty part on the right instrument is something
                // to press regenerate on, and removing it would take the sound choice with it.
                Err(error) => log::warn!("no {} was written: {error}", preset.name()),
            }
        }
        report
    }
}

/// What a written part's track is called.
///
/// The preset's own name, capitalised. Not translated: a track name is document text that travels
/// to whoever the file is sent to, and the built-in instruments and the drum grooves are already
/// named this way for the same reason.
fn part_name(preset: ClipPreset) -> String {
    let name = preset.name();
    let mut first = name.chars();
    match first.next() {
        Some(letter) => letter.to_uppercase().collect::<String>() + first.as_str(),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fixtures::{BAR, session};

    /// A session holding one four-bar melody in C major, and the clip it is in.
    fn with_a_melody() -> (Session, TrackId, ClipId) {
        let mut session = session();
        let track = session.add_default_instrument_track("Tune").unwrap();
        let clip = session
            .add_midi_clip(track, "Melody", Ticks::ZERO, BAR * 4)
            .unwrap();
        // C E G C over bar one, F A C over bar two, G B D over bar three, and home.
        let phrase: [(u8, i64, f64); 13] = [
            (60, 0, 0.0),
            (64, 0, 1.0),
            (67, 0, 2.0),
            (72, 0, 3.0),
            (65, 1, 0.0),
            (69, 1, 1.0),
            (72, 1, 2.0),
            (69, 1, 3.0),
            (67, 2, 0.0),
            (71, 2, 1.0),
            (74, 2, 2.0),
            (71, 2, 3.0),
            (60, 3, 0.0),
        ];
        for (pitch, bar, beat) in phrase {
            session
                .add_note(
                    clip,
                    Note::new(
                        pitch,
                        BAR * bar + Ticks((beat * 960.0) as i64),
                        Ticks::QUARTER,
                    ),
                )
                .unwrap();
        }
        session.forget_history();
        (session, track, clip)
    }

    #[test]
    fn a_melody_gets_a_key_a_progression_and_a_band() {
        let (mut session, track, clip) = with_a_melody();
        let before = session.midi_clip(clip).unwrap().notes.clone();

        let report = session.accompany(clip, &DEFAULT_PARTS, 1).unwrap();

        assert_eq!(report.bars, 4);
        assert_eq!(report.key.tonic.semitones(), 0, "{}", report.key.to_text());
        assert!(!report.key.is_minor());
        assert!(report.chords > 0, "no chords were written");
        assert_eq!(report.parts.len(), 3);
        assert!(report.notes > 0, "the band played nothing");

        // The key and the chords are in the document, where they can be seen and disagreed with.
        assert_eq!(session.project().harmony.key_at(Ticks::ZERO), report.key);
        assert!(session.project().harmony.numeral_at(Ticks::ZERO).is_some());

        // The melody is untouched — it is the thing being accompanied.
        assert_eq!(session.midi_clip(clip).unwrap().notes, before);
        assert_eq!(session.project().track(track).unwrap().name, "Tune");
    }

    #[test]
    fn the_whole_band_is_one_undo_step() {
        // A dozen edits — a key, a progression, three tracks and three clips — and taking them
        // back has to be one press, or somebody who did not want the accompaniment is holding
        // Undo and watching their own progression come off too.
        let (mut session, _, clip) = with_a_melody();
        let tracks = session.project().tracks.len();
        let key = session.project().harmony.key_at(Ticks::ZERO);

        session.accompany(clip, &DEFAULT_PARTS, 1).unwrap();
        assert!(session.project().tracks.len() > tracks);

        assert_eq!(session.undo(), Some(Edit::Accompany));
        assert_eq!(session.project().tracks.len(), tracks);
        assert_eq!(session.project().harmony.key_at(Ticks::ZERO), key);
        assert!(
            session.project().harmony.numeral_at(Ticks::ZERO).is_none(),
            "the progression came back with the parts"
        );
    }

    #[test]
    fn every_part_carries_the_recipe_that_lets_it_be_argued_with() {
        // The whole bargain of the feature: what it guesses is a draft, and a draft you cannot
        // rewrite is a result you are stuck with.
        let (mut session, _, clip) = with_a_melody();
        let report = session.accompany(clip, &DEFAULT_PARTS, 1).unwrap();

        for track in &report.parts {
            let clips = session
                .project()
                .track(*track)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .clips
                .clone();
            assert_eq!(clips.len(), 1);
            assert!(clips[0].is_generated(), "a part that cannot be rewritten");
        }

        // Change a chord and the parts follow it.
        let bass = session
            .project()
            .track(report.parts[0])
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips[0]
            .id;
        let first = session.midi_clip(bass).unwrap().notes.clone();
        session.set_chord(
            Ticks::ZERO,
            auris_core::theory::numeral::Numeral::new(4, false),
        );
        session.regenerate_clip(bass).unwrap();
        assert_ne!(
            session.midi_clip(bass).unwrap().notes,
            first,
            "the chord moved and the bass line did not"
        );
    }

    #[test]
    fn two_parts_written_together_are_two_different_takes() {
        // Seeded from one number and stepped, so a bass and a comp are not the same figure twice
        // — and so that pressing it again with the same seed gives the same band back.
        let (mut session, _, clip) = with_a_melody();
        let report = session
            .accompany(clip, &[ClipPreset::Bass, ClipPreset::Bass], 7)
            .unwrap();
        let notes_of = |session: &Session, track: TrackId| {
            let id = session
                .project()
                .track(track)
                .unwrap()
                .kind
                .as_instrument()
                .unwrap()
                .clips[0]
                .id;
            session.midi_clip(id).unwrap().notes.clone()
        };
        assert_ne!(
            notes_of(&session, report.parts[0]),
            notes_of(&session, report.parts[1])
        );
    }

    #[test]
    fn a_melody_that_is_not_at_the_beginning_is_accompanied_where_it_is() {
        let mut session = session();
        let track = session.add_default_instrument_track("Tune").unwrap();
        let clip = session
            .add_midi_clip(track, "Melody", BAR * 8, BAR * 2)
            .unwrap();
        for (pitch, beat) in [(60u8, 0.0), (64, 1.0), (67, 2.0), (72, 3.0)] {
            session
                .add_note(
                    clip,
                    Note::new(pitch, Ticks((beat * 960.0) as i64), Ticks::QUARTER),
                )
                .unwrap();
        }

        let report = session.accompany(clip, &[ClipPreset::Bass], 1).unwrap();
        assert_eq!(
            report.bars, 1,
            "one bar of tune is one bar of accompaniment"
        );

        // The part sits under the melody rather than at the start of the song.
        let bass = session
            .project()
            .track(report.parts[0])
            .unwrap()
            .kind
            .as_instrument()
            .unwrap()
            .clips[0]
            .clone();
        assert_eq!(bass.start, BAR * 8);
        // And the chords are written there too, with nothing before them.
        assert!(session.project().harmony.numeral_at(BAR * 8).is_some());
        assert!(session.project().harmony.numeral_at(Ticks::ZERO).is_none());
    }

    #[test]
    fn there_is_nothing_to_accompany_in_an_empty_clip() {
        let mut session = session();
        let track = session.add_default_instrument_track("Tune").unwrap();
        let clip = session
            .add_midi_clip(track, "Empty", Ticks::ZERO, BAR)
            .unwrap();
        session.forget_history();

        assert!(matches!(
            session.accompany(clip, &DEFAULT_PARTS, 1),
            Err(SessionError::NothingToAccompany(id)) if id == clip.0
        ));
        assert!(matches!(
            session.accompany(ClipId(9_999), &DEFAULT_PARTS, 1),
            Err(SessionError::UnknownClip(_))
        ));
        // A refusal costs neither an undo step nor the dirty flag.
        assert!(!session.can_undo());
        assert!(!session.is_dirty());
        assert_eq!(session.project().tracks.len(), 1);
    }

    #[test]
    fn asking_for_no_parts_still_writes_the_harmony() {
        // Reading the chords is worth having on its own: it is how somebody finds out what the
        // tune they just played is doing, and they may want to write the parts themselves.
        let (mut session, _, clip) = with_a_melody();
        let report = session.accompany(clip, &[], 1).unwrap();
        assert!(report.parts.is_empty());
        assert_eq!(report.notes, 0);
        assert!(report.chords > 0);
        assert!(!report.substituted, "no part asked for a sound");
        assert_eq!(session.project().tracks.len(), 1);
    }

    #[test]
    fn a_part_is_named_for_what_it_plays() {
        assert_eq!(part_name(ClipPreset::Bass), "Bass");
        assert_eq!(part_name(ClipPreset::Chords), "Chords");
        assert_eq!(part_name(ClipPreset::Hat), "Hat");
    }
}
