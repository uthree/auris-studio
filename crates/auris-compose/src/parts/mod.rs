//! Writing the parts.
//!
//! Each draft is a pure function of the frozen [`Frame`] and its own name. They share harmony,
//! phrase boundaries and groove. Making room for a foreground line already in the document is
//! the caller's next step, through `crate::arrangement`, before the notes are installed in a clip.
//!
//! # Where things are
//!
//! Here: what a part is — [`Draft`], [`PartDraft`] and [`ScoreSettings`] — the roster loop in
//! [`write_parts`], and the three passes that pick the notes up again once a writer has put them
//! down. `shorten` and `swing` are post-passes rather than each writer's business so that one
//! setting means the same thing in every part, and for the same reason the tests that hold for
//! the whole band are here rather than in any one writer's file. `untangle` is a post-pass because
//! it has to run *after* the other two: it clears up what moving a start without moving a length
//! leaves behind, so no writer could do it for itself. The feel that is *not* written — the
//! wander and the per-role lean — is installed on the finished clips instead; [`crate::perform`]
//! is that table.
//!
//! One file per role — `melody`, `comp`, `arp`, `bass` and `drums` — because not one of them
//! calls another. They are spokes, and somebody reading how a bass line is built should not have
//! to read the drummer to do it. What they do all read is `writer`, the seven helpers they share
//! and nothing else, and `crate::fixture`, which is the same arrangement for what their tests are
//! written against.

mod arp;
mod bass;
mod comp;
mod drums;
mod melody;
mod writer;

use auris_core::time::Ticks;

use crate::frame::Frame;
use crate::rhythm::{SwingFeel, swing_offset};
use crate::spec::{Mood, PartSpec, Role};

use arp::arp;
use bass::bass;
use comp::comp;
use drums::drums;
use melody::melody;
use writer::part_grid;

/// A note as the composer writes it, before it becomes a clip.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Draft {
    /// Which section it belongs to.
    pub section: usize,
    /// MIDI pitch.
    pub pitch: u8,
    /// How hard it is struck, from 0 to 1.
    pub velocity: f32,
    /// Where it starts, from the beginning of the song.
    pub start: Ticks,
    /// How long it sounds.
    pub length: Ticks,
}

/// Everything one part plays.
#[derive(Clone, Debug)]
pub struct PartDraft {
    /// The part's name, which a drum kit keeps as each note's voice.
    pub name: String,
    /// The notes, in time order.
    pub notes: Vec<Draft>,
}

/// How a part is played, as opposed to what it plays.
///
/// The dials the writers read that are neither the harmony nor the part itself.
/// They arrive separately from the part so that one recipe's dials reach every part a clip
/// preset writes — the three voices of a kit play one groove at one swing.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreSettings {
    /// Musical vocabulary, independent of the clip's performance transforms.
    pub style: Option<crate::PerformanceStyle>,
    /// How the music should feel, which sets density and syncopation.
    pub mood: Mood,
    /// How far the offbeats are delayed, as a percentage where 50 is straight.
    ///
    /// The one piece of feel still written into the notes. The wander and the lean moved to the
    /// performance stack ([`crate::perform`]) where a person can turn them without a rewrite;
    /// the swing stays here because the *groove* decides which pairs it delays, and a groove is
    /// something only the writers know.
    pub swing: u8,
    /// How far apart the hardest and softest notes are struck, from 0 to 1.
    ///
    /// Distinct from how hard the part is played, which is the section's intensity: this is how
    /// much the playing varies *around* that. It is the one dial the metric hierarchy answers to,
    /// so it reaches every accent and every phrase shape rather than one writer's idea of them.
    pub dynamics: f32,
    /// How much of a section's last bar the snare runs as a fill, from 0 to 1.
    pub fill: f32,
    /// Which drum groove the rhythm section plays.
    pub groove: String,
    /// The tune's contour, when it was given rather than left to the seed.
    ///
    /// Scale steps around the figure's anchor; empty means the melody draws its own germ.
    pub motif: Vec<i32>,
}

/// Writes every part of a roster against a frame.
pub fn write_parts(settings: &ScoreSettings, roster: &[PartSpec], frame: &Frame) -> Vec<PartDraft> {
    roster
        .iter()
        .map(|part| {
            let mut draft = PartDraft {
                name: part.name.clone(),
                notes: Vec::new(),
            };
            for (index, section) in frame.sections.iter().enumerate() {
                // The accompaniment plays its own figure in every bar, not once per chord: a
                // chord held over a bar line is one chord in the chart, and still two bars of
                // comping. See `SectionPlan::bar_by_bar`.
                let barred = || section.bar_by_bar(frame.grid.bar_ticks());
                let notes = match part.role {
                    Role::Melody => melody(settings, frame, section, index, part),
                    Role::Chords | Role::Pad => comp(settings, frame, &barred(), index, part),
                    Role::Arp => arp(settings, frame, &barred(), index, part),
                    Role::Bass => bass(settings, frame, &barred(), index, part),
                    Role::Kick | Role::Snare | Role::Hat => {
                        drums(settings, frame, section, index, part)
                    }
                };
                draft.notes.extend(notes);
            }
            shorten(part, &mut draft.notes);
            swing(settings, frame, part, &mut draft.notes);
            draft
                .notes
                .sort_by_key(|note| (note.start.raw(), note.pitch));
            untangle(&mut draft.notes);
            draft
        })
        .collect()
}

/// The shortest a gate is allowed to cut a note to. Below this it is a click rather than a pitch.
const MIN_NOTE_TICKS: i64 = 30;

/// The lowest the gate goes: a twentieth of the gap, which is already a staccatissimo.
const MIN_GATE: f32 = 0.05;

/// Cuts every note back to its share of the gap to the one after it.
///
/// Applied here rather than inside each writer so that one setting means the same thing in every
/// part. Each writer has already decided a note's length by where the next note starts; the gate
/// says how much of that the note actually sounds for, which is the difference between a chord
/// struck sixteen times a bar and a chord held for one.
///
/// A drum is left alone. A one-shot ignores its note-off, so shortening one would change nothing
/// anybody can hear and only make the piano roll harder to read.
fn shorten(part: &PartSpec, notes: &mut [Draft]) {
    let gate = part.gate.clamp(MIN_GATE, 1.0);
    if part.role.is_drum() || gate >= 1.0 {
        return;
    }
    for note in notes.iter_mut() {
        // The floor never lengthens a note: a chord shorter than the floor to begin with is a
        // chord the harmony asked for, and the gate is not the place to argue with it.
        let floor = MIN_NOTE_TICKS.min(note.length.raw()).max(1);
        let shortened = (note.length.raw() as f32 * gate).round() as i64;
        note.length = Ticks(shortened.max(floor));
    }
}

/// Swings the timing, so the part plays the groove's feel rather than the grid.
///
/// This used to be the humanisation too — a wander over the timing and the velocity, and a
/// constant per-role lean. Both moved off the notes and onto the clip's performance stack
/// ([`crate::perform`] is the table of what each part starts with), because they are how the
/// text is *played*, and a feel written into the text could neither be turned without a rewrite
/// nor differ from one loop pass to the next. The swing stays, and stays written: which pairs it
/// delays is the groove's own answer — a kit running sixteenths swings them, a kit in eighths
/// swings those — and the groove is something only the writers know.
///
/// `swing: 50` is exactly the identity, which is what lets every timing test assert on an exact
/// tick rather than on a tolerance.
fn swing(settings: &ScoreSettings, frame: &Frame, part: &PartSpec, notes: &mut [Draft]) {
    // The whole band reads the groove's one answer — a comp swinging pairs the kit does not
    // would be two feels at once.
    let feel = crate::rhythm::groove(&settings.groove)
        .map_or(SwingFeel::Eighth, crate::rhythm::Groove::swing_feel);

    // The grid the notes were written on: a part on triplets has its offbeats on other steps.
    let grid = part_grid(frame, part);
    for note in notes.iter_mut() {
        let bar_position = note.start.raw().rem_euclid(grid.bar_ticks().raw().max(1));
        let step = grid.step_of(Ticks(bar_position));
        note.start = (note.start + swing_offset(grid, step, settings.swing, feel)).max_zero();
    }
}

/// Cuts every note back to where the next note of the same pitch begins.
///
/// A writer sets a note's length from where the *next* note starts, so nothing it writes overlaps
/// itself. Then `swing` moves the starts and leaves the lengths alone — an offbeat note is
/// delayed past the end of its own bar-mate — and a note now ends after the next note of its own
/// pitch has already begun. Measured over the eight presets at four seeds each, 41,184 notes:
/// 0.8 per cent of them, back when this pass also carried the wander it was 13. The wander now
/// happens at performance time, where an overlap it opens meets the same release-first rule
/// below rather than a cut — the text cannot be trimmed against an accident that redraws itself
/// every pass.
///
/// What that costs depends on the instrument, which is exactly why the composer must not write
/// one. A note-off names a pitch and not a note, so an instrument meeting two of them has to
/// choose which it ends, and the workspace's own answer is written down for it: release the one
/// that started first — `auris_session::guide`, and `auris_synth::VoiceAllocator::note_off` for
/// the implementation the built-in voices share.
///
/// That is the answer here and not everywhere. Seven of the eight presets play through a
/// SoundFont, where a note-off reaches the font's own synthesiser on one channel and the library
/// decides; a hosted CLAP plugin may do whatever it does. Neither can be asked, so the question
/// is better not put.
///
/// The cut lands *exactly* on the next note's start rather than a tick before it, so a repeated
/// note stays legato. That is safe because both places that read these notes put releases first
/// where they tie — `graph::schedule::event_rank` for playback, and the MIDI writer's own sort for
/// export — and both say so for this reason.
///
/// `notes` must be sorted by start, which is what makes the reverse walk find the *nearest*
/// following note of a pitch. Two notes of one pitch struck at the same tick are left alone: there
/// is nothing to cut back to, and shortening one to a tick would turn a doubled note into a click.
fn untangle(notes: &mut [Draft]) {
    let mut next_start = [None; 256];
    for note in notes.iter_mut().rev() {
        let slot = &mut next_start[usize::from(note.pitch)];
        if let Some(next) = *slot
            && next > note.start
            && note.start + note.length > next
        {
            note.length = next - note.start;
        }
        *slot = Some(note.start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Song, part};
    use crate::rhythm::Pattern;
    use crate::theory::pitch::PitchClass;
    use auris_core::Subdivision;

    /// A one-bar kick playing every sixteenth, at a swing of 66 over `groove`.
    fn swung_kick(groove: &str) -> Vec<i64> {
        let mut song = Song::with_parts([Song::part(Role::Kick, |kick| {
            kick.rhythm = Pattern::parse("x x x x x x x x x x x x x x x x");
        })]);
        song.bars = 1;
        song.settings.swing = 66;
        song.settings.groove = groove.to_string();
        let (_, parts) = song.draft();
        part(&parts, "kick")
            .notes
            .iter()
            .map(|note| note.start.raw())
            .collect()
    }

    #[test]
    fn a_sixteen_beat_groove_swings_its_sixteenth_pairs() {
        // The swing dial under a groove that runs sixteenths. Felt at the eighth it moved the
        // third and fourth sixteenth of every beat together, so the gap into the next beat gave
        // up the whole shift, where a sixteen-beat drummer plays long-short pairs. The groove
        // answers which pairs swing, and every odd sixteenth is late by the same 77 ticks:
        // long-short, the gap into each beat the short one.
        let expected: Vec<i64> = (0..16)
            .map(|step| step * 240 + if step % 2 == 1 { 77 } else { 0 })
            .collect();
        assert_eq!(swung_kick("sixteen-beat"), expected);

        // And the same dial over a groove in eighths still swings the eighth, sixteenths riding
        // with it — which is what swing has always meant there.
        let expected: Vec<i64> = (0..16)
            .map(|step| step * 240 + if step % 4 >= 2 { 154 } else { 0 })
            .collect();
        assert_eq!(swung_kick("eight-beat"), expected);
    }

    #[test]
    fn a_triplet_part_is_swung_on_its_own_grid() {
        // `swing` looks a note's step up on a grid to decide whether it is an offbeat worth
        // delaying. Read off a sixteenth grid, a part on triplets would have the wrong steps of
        // the bar moved.
        let mut song = Song::with_parts([Song::part(Role::Melody, |lead| {
            lead.subdivision = Subdivision::EighthTriplet;
            lead.density = Some(0.9);
        })]);
        song.settings.swing = 66;
        song.seed = 5;
        let (_, parts) = song.draft();
        // A triplet grid has nothing for swing to do — the offbeat is already where the dial
        // would push it — so every note lands exactly on a third of a beat.
        for note in &part(&parts, "melody").notes {
            assert_eq!(
                note.start.raw() % 320,
                0,
                "a note at {} is not on a triplet",
                note.start.raw()
            );
        }
    }

    #[test]
    fn every_default_part_writes_notes() {
        let (_, parts) = Song::default().draft();
        assert_eq!(parts.len(), 6);
        for part in &parts {
            assert!(!part.notes.is_empty(), "`{}` wrote nothing", part.name);
        }
    }

    #[test]
    fn notes_stay_inside_their_parts_range() {
        let song = Song::default();
        let (_, parts) = song.draft();
        for (draft, declared) in parts.iter().zip(&song.parts) {
            if declared.role.is_drum() {
                continue;
            }
            let (low, high) = declared.range();
            for note in &draft.notes {
                assert!(
                    (low..=high).contains(&i32::from(note.pitch)),
                    "`{}` played {} outside {low}..{high}",
                    draft.name,
                    note.pitch
                );
            }
        }
    }

    /// Every pitched note of `song` is in the scale, its chord, or the chord it anticipates.
    fn assert_in_scale_or_chord(song: &Song) {
        let (frame, parts) = song.draft();
        let section = &frame.sections[0];
        for (draft, declared) in parts.iter().zip(&song.parts) {
            if declared.role.is_drum() {
                continue;
            }
            for note in &draft.notes {
                let class = PitchClass::new(i32::from(note.pitch));
                let chord = section.chord_at(note.start);
                let in_scale = section.key.scale.contains(section.key.tonic, class);
                let in_chord = chord.is_some_and(|event| event.chord.contains(class));
                // A pushed chord is struck before its own start and judged by the chord it
                // anticipates, which is the one sounding where the note *ends*.
                let anticipates = section
                    .chord_at(note.start + note.length - Ticks(1))
                    .is_some_and(|event| event.chord.contains(class));
                assert!(
                    in_scale || in_chord || anticipates,
                    "`{}` played {class} over {} in `{}`",
                    draft.name,
                    chord.map(|e| e.chord.to_string()).unwrap_or_default(),
                    song.chords
                );
            }
        }
    }

    #[test]
    fn every_pitched_note_belongs_to_the_key() {
        // Not every note has to be a chord tone, but a note outside the scale is a wrong note.
        assert_in_scale_or_chord(&Song::default());
    }

    #[test]
    fn no_note_starts_before_the_stretch_or_runs_past_it() {
        let (frame, parts) = Song::default().draft();
        for draft in &parts {
            for note in &draft.notes {
                assert!(note.start >= Ticks::ZERO, "`{}` started early", draft.name);
                assert!(
                    note.start < frame.length,
                    "`{}` started past the end",
                    draft.name
                );
                assert!(
                    note.length > Ticks::ZERO,
                    "`{}` wrote a zero-length note",
                    draft.name
                );
            }
        }
    }

    #[test]
    fn unswung_every_note_lands_exactly_on_the_grid() {
        // The text is the score: nothing the writers place sits off the grid unless the swing
        // put it there. The wander lives on the performance stack.
        let (frame, parts) = Song::default().draft();
        let step = frame.grid.step_ticks().raw();
        for draft in &parts {
            for note in &draft.notes {
                assert_eq!(
                    note.start.raw() % step,
                    0,
                    "`{}` placed a note off the grid at {}",
                    draft.name,
                    note.start.raw()
                );
            }
        }
    }

    #[test]
    fn swing_delays_the_offbeats_of_a_busy_part() {
        let straight = Song::default().draft().1;
        let mut swung = Song::default();
        swung.settings.swing = 66;
        let swung = swung.draft().1;
        let hat_straight = part(&straight, "hat");
        let hat_swung = part(&swung, "hat");
        let delayed = hat_straight
            .notes
            .iter()
            .zip(&hat_swung.notes)
            .filter(|(a, b)| b.start > a.start)
            .count();
        assert!(delayed > 0, "swing moved nothing");
        assert!(
            hat_straight
                .notes
                .iter()
                .zip(&hat_swung.notes)
                .all(|(a, b)| b.start >= a.start),
            "swing must never rush a note"
        );
    }

    #[test]
    fn nothing_answers_a_borrowed_note_with_the_degree_it_replaced() {
        // A secondary dominant raises a degree; a part still drawing on the key's own scale goes
        // on playing the unraised one, and both versions sound at once. That is not colour, it is
        // the one dissonance an ear calls a mistake — the melody answered the G7 of a minor-key
        // 丸サ進行 with a B flat, a semitone under the chord's own third.
        for (key, chart) in [
            ("C minor", "@marusa"),
            ("C major", "@marusa"),
            ("A minor", "@royal-road"),
            ("Eb major", "@naki"),
            ("F# minor", "@junjo"),
        ] {
            // Straight, so a note struck on a chord change is heard where it was chosen. This
            // is about which note a part picks, not about when it arrives.
            let song = Song {
                key,
                chords: chart.to_string(),
                bars: 8,
                seed: 3,
                ..Song::default()
            };
            let (frame, parts) = song.draft();
            let section = &frame.sections[0];
            for (part_draft, declared) in parts.iter().zip(&song.parts) {
                if declared.role.is_drum() {
                    continue;
                }
                for note in &part_draft.notes {
                    let Some(event) = section.chord_at(note.start) else {
                        continue;
                    };
                    let class = PitchClass::new(i32::from(note.pitch));
                    if event.chord.contains(class) {
                        continue;
                    }
                    // A pushed chord belongs to the change it anticipates, not to the chord it
                    // is struck over.
                    if section
                        .chord_at(note.start + note.length - Ticks(1))
                        .is_some_and(|arriving| arriving.chord.contains(class))
                    {
                        continue;
                    }
                    for tone in event.chord.classes() {
                        // Only a chord tone the key does not have: a semitone between two notes
                        // the key itself offers is ordinary tension and stays.
                        if event.key.scale.contains(event.key.tonic, tone) {
                            continue;
                        }
                        let apart = class.distance_up_to(tone).min(tone.distance_up_to(class));
                        assert_ne!(
                            apart, 1,
                            "`{}` played {class} a semitone from the {tone} of {} in {key} \
                             {chart}",
                            part_draft.name, event.chord
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn no_part_plays_a_note_outside_the_scale_or_the_chord() {
        // The charts deliberately contain a diminished triad and a slash chord, which is where
        // a bass line that assumed a perfect fifth above the sounding bass went wrong.
        for chart in [
            "| I | vii | I | V |",
            "@koakuma",
            "@marusa",
            "@junjo",
            "@blues",
        ] {
            assert_in_scale_or_chord(&Song {
                chords: chart.to_string(),
                ..Song::default()
            });
        }
    }

    #[test]
    fn adding_a_part_leaves_the_other_parts_alone() {
        // Every part hangs off the same skeleton, so taking that skeleton from whichever melody
        // part happened to be in the roster meant adding a part rewrote the whole arrangement.
        let roster = [
            PartSpec::of_role("bass", Role::Bass),
            PartSpec::of_role("kick", Role::Kick),
        ];
        let before = Song::with_parts(roster.clone()).draft().1;
        let after = Song::with_parts(
            roster
                .into_iter()
                .chain([PartSpec::of_role("extra", Role::Pad)]),
        )
        .draft()
        .1;
        for name in ["bass", "kick"] {
            assert_eq!(
                part(&before, name).notes,
                part(&after, name).notes,
                "adding a part rewrote `{name}`"
            );
        }
    }

    #[test]
    fn a_written_rhythm_reaches_every_pitched_part() {
        // The field's contract is that it overrides the generated rhythm. The melody and the
        // drums kept that promise; the chords, the pad, the arp and the bass read the field and
        // played their own rhythm anyway.
        for role in [Role::Chords, Role::Pad, Role::Arp, Role::Bass] {
            let mut song = Song::with_parts([Song::part(role, |part| {
                part.rhythm = Pattern::parse("x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~");
            })]);
            song.bars = 2;
            let (frame, parts) = song.draft();
            let played = part(&parts, role.name());
            assert!(
                !played.notes.is_empty(),
                "{} wrote nothing at all",
                role.name()
            );
            let beat = frame.grid.bar_ticks().raw() / 4;
            for note in &played.notes {
                assert!(
                    note.start.raw() % beat == 0,
                    "{} struck tick {}, off the written rhythm",
                    role.name(),
                    note.start.raw()
                );
            }
        }
    }

    /// The lead, the chords, the bass and the kick at seed 5, with `patch` applied to the chords.
    fn roster(patch: impl FnOnce(&mut PartSpec)) -> Vec<PartDraft> {
        let mut song = Song::with_parts([
            PartSpec::of_role("lead", Role::Melody),
            Song::part(Role::Chords, patch),
            PartSpec::of_role("bass", Role::Bass),
            PartSpec::of_role("kick", Role::Kick),
        ]);
        song.seed = 5;
        song.draft().1
    }

    #[test]
    fn a_subdivision_and_a_gate_reach_only_the_part_that_asked_for_them() {
        // Both live on the part, so turning them up must leave every other part where it was.
        let before = roster(|_| {});
        let after = roster(|chords| {
            chords.subdivision = Subdivision::SixteenthTriplet;
            chords.gate = 0.25;
        });
        for name in ["lead", "bass", "kick"] {
            assert_eq!(
                part(&before, name).notes,
                part(&after, name).notes,
                "changing the chords rewrote `{name}`"
            );
        }
        assert_ne!(
            part(&before, "chords").notes,
            part(&after, "chords").notes,
            "the settings reached nothing"
        );
    }

    #[test]
    fn the_gate_shortens_a_note_without_moving_it() {
        // Articulation, not rhythm. A gate that shifted a note would be a second timing control
        // fighting the swing for the same tick.
        let long = roster(|_| {});
        let short = roster(|chords| chords.gate = 0.25);
        let (long, short) = (part(&long, "chords"), part(&short, "chords"));
        assert_eq!(long.notes.len(), short.notes.len());

        let mut shortened = 0;
        for (a, b) in long.notes.iter().zip(&short.notes) {
            assert_eq!(a.start, b.start, "the gate moved a note");
            assert_eq!(a.pitch, b.pitch);
            assert!(b.length <= a.length, "the gate lengthened a note");
            assert!(b.length > Ticks::ZERO, "the gate silenced a note");
            if b.length < a.length {
                shortened += 1;
            }
        }
        assert!(shortened > 0, "the gate shortened nothing");
    }

    #[test]
    fn the_same_song_writes_the_same_notes_every_time() {
        let first = Song::default().draft().1;
        let second = Song::default().draft().1;
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.notes, b.notes, "`{}` is not deterministic", a.name);
        }
    }

    #[test]
    fn a_different_seed_writes_a_different_take() {
        let seeded = |seed| {
            Song {
                seed,
                ..Song::default()
            }
            .draft()
            .1
        };
        let (a, b) = (seeded(1), seeded(2));
        assert_ne!(
            part(&a, "lead").notes,
            part(&b, "lead").notes,
            "the seed did not reach the melody"
        );
    }

    #[test]
    fn a_note_is_cut_back_to_where_its_pitch_is_struck_again_and_no_further() {
        let note = |pitch, start, length| Draft {
            section: 0,
            pitch,
            velocity: 0.5,
            start: Ticks(start),
            length: Ticks(length),
        };
        // In the order `write_parts` hands them over: by start, then by pitch.
        let mut notes = vec![
            note(60, 0, 500),
            note(64, 0, 500),
            note(60, 480, 480),
            note(64, 960, 480),
        ];
        untangle(&mut notes);
        assert_eq!(
            notes[0].length,
            Ticks(480),
            "cut to the restrike, and to it exactly"
        );
        assert_eq!(
            notes[1].length,
            Ticks(500),
            "another pitch is another voice"
        );
        assert_eq!(
            notes[2].length,
            Ticks(480),
            "the last of a pitch keeps its length"
        );
        assert_eq!(notes[3].length, Ticks(480));

        // Two struck on the same tick are a doubled note and not an overlap: there is nothing to
        // cut back to, and cutting one to a tick would turn it into a click.
        let mut doubled = vec![note(60, 0, 480), note(60, 0, 480)];
        untangle(&mut doubled);
        assert_eq!(doubled[0].length, Ticks(480));
        assert_eq!(doubled[1].length, Ticks(480));
    }

    #[test]
    fn nothing_is_left_sounding_when_its_own_pitch_is_struck_again() {
        // A note-off names a pitch and not a note, so two notes of one pitch overlapping is a
        // question the writer is asking the instrument rather than answering — and most of the
        // instruments a clip reaches are a SoundFont library or somebody else's plugin, neither
        // of which can be asked. The swing used to write them on its own.
        for groove in ["eight-beat", "sixteen-beat", "basic-rock"] {
            for seed in 0..4u64 {
                let mut song = Song {
                    seed,
                    bars: 8,
                    ..Song::default()
                };
                song.settings.swing = 66;
                song.settings.groove = groove.to_string();
                for draft in song.draft().1 {
                    let mut sounding = std::collections::BTreeMap::new();
                    for note in &draft.notes {
                        let Some(ends) = sounding.insert(note.pitch, note.start + note.length)
                        else {
                            continue;
                        };
                        assert!(
                            ends <= note.start,
                            "{groove} seed {seed}: in `{}` a {} sounding to {} is struck again \
                             at {}",
                            draft.name,
                            note.pitch,
                            ends.raw(),
                            note.start.raw(),
                        );
                    }
                }
            }
        }
    }
}
