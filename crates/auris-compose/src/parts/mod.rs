//! Writing the parts.
//!
//! Every part is a pure function of the frozen [`Frame`] and its own name,
//! so no part can depend on another's notes. What makes them sound like a band anyway is that
//! they all read the same harmony, and the rhythm section all reads the same groove.
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
//! and nothing else, and `fixture`, which is the same arrangement for what their tests are
//! written against.

mod arp;
mod bass;
mod coda;
mod comp;
mod drums;
mod joins;
mod melody;
mod writer;

#[cfg(test)]
mod fixture;

use auris_core::time::Ticks;

use crate::frame::Frame;
use crate::rhythm::{SwingFeel, swing_offset};
use crate::spec::{Mood, PartSpec, Role, SongSpec};

use arp::arp;
use bass::bass;
use coda::coda;
use comp::comp;
use drums::drums;
use joins::{joins, riser};
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
    /// The part's name, which becomes its track name.
    pub name: String,
    /// The plugin that plays it, when no [`Self::sound`] names a SoundFont one.
    pub instrument: String,
    /// The General MIDI sound it asked for, if it asked for one.
    pub sound: Option<crate::gm::Sound>,
    /// Level trim.
    pub gain_db: f32,
    /// Stereo position.
    pub pan: f32,
    /// The notes, in time order.
    pub notes: Vec<Draft>,
}

/// How a part is played, as opposed to what it plays.
///
/// The dials the writers read that are neither the harmony, the form, nor the part itself.
/// They arrive separately from a [`SongSpec`] so that a caller who has no specification — one
/// regenerating a single clip against the harmony already in a document — can still ask for a
/// part without inventing a whole song around it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScoreSettings {
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
    /// How much a repeat departs from what the section played the first time.
    pub variation: f32,
    /// Which drum groove the rhythm section plays.
    pub groove: String,
    /// The tune's contour, when it was given rather than left to the seed.
    ///
    /// Scale steps around the figure's anchor; empty means the melody draws its own germ. See
    /// [`SongSpec::motif`] for what a given motif is and is not.
    pub motif: Vec<i32>,
}

impl From<&SongSpec> for ScoreSettings {
    fn from(spec: &SongSpec) -> Self {
        Self {
            mood: spec.mood,
            swing: spec.swing,
            dynamics: spec.dynamics,
            fill: spec.fill,
            variation: spec.variation,
            groove: spec.groove.clone(),
            motif: spec.motif.clone(),
        }
    }
}

/// Writes every part of a roster against a frame.
pub fn write_parts(settings: &ScoreSettings, roster: &[PartSpec], frame: &Frame) -> Vec<PartDraft> {
    roster
        .iter()
        .map(|part| {
            let mut draft = PartDraft {
                name: part.name.clone(),
                instrument: part.instrument.clone(),
                sound: part.sound(),
                gain_db: part.gain_db,
                pan: part.pan,
                notes: Vec::new(),
            };
            // The part as each section plays it, resolved once for the whole part. A section may
            // patch how it plays — busier, an octave up, on sixteenths — and *every* pass below
            // has to read the same answer: a writer taking the chorus's subdivision while the
            // gate and the swing afterwards took the roster's would be one part played two ways
            // at once, and the seam would show as notes that do not line up with themselves.
            let played: Vec<PartSpec> = frame
                .sections
                .iter()
                .map(|plan| plan.played(part))
                .collect();

            for (index, section) in frame.sections.iter().enumerate() {
                if !section.parts.is_empty() && !section.parts.contains(&part.name) {
                    continue;
                }
                let part = &played[index];
                // The held final bar has a writer of its own: an ending is not one more bar of
                // figures and grooves, it is the piece landing. The crash still goes through
                // `joins`, because the ending is an arrival and that writer's whole question is
                // whether one has happened.
                if section.coda && !matches!(part.role, Role::Crash | Role::Riser) {
                    draft.notes.extend(coda(settings, section, index, part));
                    continue;
                }
                let notes = match part.role {
                    Role::Melody => melody(settings, frame, section, index, part),
                    Role::Chords | Role::Pad | Role::Stab => {
                        comp(settings, frame, section, index, part)
                    }
                    Role::Arp => arp(settings, frame, section, index, part),
                    Role::Bass => bass(settings, frame, section, index, part),
                    // Written against the joins of the form rather than against a groove: each is
                    // handed a section and asks whether an arrival is worth marking, which is a
                    // question no bar-long pattern can answer. The crash marks the arrival
                    // itself; the riser announces the one coming, from the bar before it.
                    Role::Crash => joins(settings, frame, section, index, part),
                    Role::Riser => riser(settings, frame, section, index, part),
                    Role::Kick | Role::Snare | Role::Hat => {
                        drums(settings, frame, section, index, part)
                    }
                };
                draft.notes.extend(notes);
            }
            shorten(&played, &mut draft.notes);
            swing(settings, frame, &played, &mut draft.notes);
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
///
/// `played` is the part as each section plays it, so a gate a section patches reaches the notes of
/// that section and no others.
fn shorten(played: &[PartSpec], notes: &mut [Draft]) {
    for note in notes.iter_mut() {
        let Some(part) = played.get(note.section) else {
            continue;
        };
        if part.role.is_drum() || part.role == Role::Riser {
            continue;
        }
        let gate = part.gate.clamp(MIN_GATE, 1.0);
        if gate >= 1.0 {
            continue;
        }
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
fn swing(settings: &ScoreSettings, frame: &Frame, played: &[PartSpec], notes: &mut [Draft]) {
    let Some(part) = played.first() else {
        return;
    };
    // A swell does not swing. The riser's start is measured back from the join in seconds, and
    // wherever that lands is where the sample has to begin for its peak to arrive on time — a
    // start that happened to fall on an offbeat step would otherwise be delayed like one.
    if part.role == Role::Riser {
        return;
    }
    // The whole band reads the groove's one answer — a comp swinging pairs the kit does not
    // would be two feels at once.
    let feel = crate::rhythm::groove(&settings.groove)
        .map_or(SwingFeel::Eighth, crate::rhythm::Groove::swing_feel);

    for note in notes.iter_mut() {
        // The grid this note was written on, which is the section's and not the roster's: a part
        // put onto triplets for one section would otherwise have its swing looked up on a
        // sixteenth grid, and the offbeat it delayed would be the wrong step of the bar.
        let grid = part_grid(frame, played.get(note.section).unwrap_or(part));
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
    use super::fixture::{BASE, draft, part, roster, section_body, section_notes};
    use super::*;
    use crate::theory::pitch::PitchClass;

    #[test]
    fn a_sixteen_beat_groove_swings_its_sixteenth_pairs() {
        // The swing dial under a groove that runs sixteenths. Felt at the eighth it moved the
        // third and fourth sixteenth of every beat together, so the gap into the next beat gave
        // up the whole shift — measured at the city-pop preset's own dial, hat gaps of
        // 142/176/142/107 ms where a sixteen-beat drummer plays long-short pairs. The groove now
        // answers which pairs swing, and every odd sixteenth is late by the same 77 ticks:
        // long-short, the gap into each beat the short one.
        let (_, _, parts) = draft(
            r#"
            form = "verse"
            swing = 66
            groove = "sixteen-beat"
            ending = "none"
            [section.verse]
            bars = 1
            [[part]]
            name = "kick"
            rhythm = "x x x x x x x x x x x x x x x x"
            "#,
        );
        let starts: Vec<i64> = part(&parts, "kick")
            .notes
            .iter()
            .map(|note| note.start.raw())
            .collect();
        let expected: Vec<i64> = (0..16)
            .map(|step| step * 240 + if step % 2 == 1 { 77 } else { 0 })
            .collect();
        assert_eq!(starts, expected);

        // And the same dial over a groove in eighths still swings the eighth, sixteenths riding
        // with it — which is what swing has always meant there.
        let (_, _, parts) = draft(
            r#"
            form = "verse"
            swing = 66
            groove = "eight-beat"
            ending = "none"
            [section.verse]
            bars = 1
            [[part]]
            name = "kick"
            rhythm = "x x x x x x x x x x x x x x x x"
            "#,
        );
        let starts: Vec<i64> = part(&parts, "kick")
            .notes
            .iter()
            .map(|note| note.start.raw())
            .collect();
        let expected: Vec<i64> = (0..16)
            .map(|step| step * 240 + if step % 4 >= 2 { 154 } else { 0 })
            .collect();
        assert_eq!(starts, expected);
    }

    /// The default roster over a two-section form, with `lead` patched in the chorus.
    fn tweaked(lines: &str) -> (Frame, Vec<PartDraft>) {
        let (_, frame, parts) = draft(&format!(
            r#"
            form     = "verse chorus"
            chords   = "@axis"
            seed     = 5

            [section.verse]
            bars = 4
            [section.chorus]
            bars = 4

            [section.chorus.part.lead]
            {lines}
            "#
        ));
        (frame, parts)
    }

    /// One part's notes in one section.
    fn in_section(draft: &PartDraft, section: usize) -> Vec<Draft> {
        draft
            .notes
            .iter()
            .filter(|note| note.section == section)
            .copied()
            .collect()
    }

    #[test]
    fn a_section_can_send_a_part_an_octave_up_without_moving_it_anywhere_else() {
        let (_, parts) = tweaked("octave = 6");
        let lead = part(&parts, "lead");
        let low = in_section(lead, 0).iter().map(|n| n.pitch).min().unwrap();
        let high = in_section(lead, 1).iter().map(|n| n.pitch).min().unwrap();
        assert!(
            high > low,
            "the chorus sits at {high} against the verse's {low}"
        );
        // The verse is what it was without the tweak, which is the half that says this is a patch
        // on one section rather than a change to the part.
        let (_, plain) = tweaked("density = 0.5");
        assert_eq!(
            in_section(lead, 0),
            in_section(part(&plain, "lead"), 0),
            "the tweak reached back into the verse"
        );
    }

    #[test]
    fn a_gate_a_section_patches_reaches_that_sections_notes_and_no_others() {
        // The trap the whole resolution exists for. `shorten` runs once over the finished part,
        // after every section has been written, so a gate read off the roster there would apply
        // the verse's value to the chorus's notes — the one place a per-section field silently
        // does nothing.
        let (_, parts) = tweaked("gate = 0.25");
        let lead = part(&parts, "lead");
        let mean = |notes: &[Draft]| {
            notes.iter().map(|n| n.length.raw()).sum::<i64>() / notes.len().max(1) as i64
        };
        let verse = mean(&in_section(lead, 0));
        let chorus = mean(&in_section(lead, 1));
        assert!(
            chorus * 2 < verse,
            "a quarter of the gap should be well under half: {chorus} against {verse}"
        );
    }

    #[test]
    fn a_subdivision_a_section_patches_is_swung_on_its_own_grid() {
        // The other half of the same trap, and the subtler one: `swing` looks a note's step up
        // on a grid to decide whether it is an offbeat worth delaying. Read off the roster, a
        // section put onto triplets would have its swing measured against sixteenths and the
        // wrong steps of the bar would move.
        let (_, _, parts) = draft(
            r#"
            form     = "verse chorus"
            chords   = "@axis"
            swing    = 66
            seed     = 5

            [section.verse]
            bars = 4
            [section.chorus]
            bars = 4

            [section.chorus.part.lead]
            subdivision = "8t"
            density     = 0.9
            "#,
        );
        let lead = part(&parts, "lead");
        // A triplet grid has nothing for swing to do — the offbeat is already where the dial
        // would push it — so every note of that section lands exactly on a third of a beat.
        for note in in_section(lead, 1) {
            assert_eq!(
                note.start.raw() % 320,
                0,
                "a chorus note at {} is not on a triplet",
                note.start.raw()
            );
        }
    }

    #[test]
    fn every_default_part_writes_notes() {
        let (_, _, parts) = draft(BASE);
        assert_eq!(parts.len(), 6);
        for part in &parts {
            assert!(!part.notes.is_empty(), "`{}` wrote nothing", part.name);
        }
    }

    #[test]
    fn notes_stay_inside_their_parts_range() {
        let (spec, _, parts) = draft(BASE);
        for (draft, declared) in parts.iter().zip(&spec.parts) {
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

    #[test]
    fn every_pitched_note_belongs_to_the_key() {
        // Not every note has to be a chord tone, but a note outside the scale is a wrong note.
        let (spec, frame, parts) = draft(BASE);
        let section = &frame.sections[0];
        for (draft, declared) in parts.iter().zip(&spec.parts) {
            if declared.role.is_drum() {
                continue;
            }
            for note in &draft.notes {
                let class = PitchClass::new(i32::from(note.pitch));
                let in_scale = section.key.scale.contains(section.key.tonic, class);
                let in_chord = section
                    .chord_at(note.start - section.start)
                    .is_some_and(|event| event.chord.contains(class));
                // A pushed chord is struck before its own start and judged by the chord it
                // anticipates, which is the one sounding where the note *ends*.
                let anticipates = section
                    .chord_at(note.start + note.length - section.start - Ticks(1))
                    .is_some_and(|event| event.chord.contains(class));
                assert!(
                    in_scale || in_chord || anticipates,
                    "`{}` played {class} which is in neither the scale nor the chord",
                    draft.name
                );
            }
        }
    }

    #[test]
    fn no_note_starts_before_the_song_or_runs_past_it() {
        let (_, frame, parts) = draft(BASE);
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
        // put it there. The wander that used to blur this lives on the performance stack now.
        let (_, frame, parts) = draft(BASE);
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
        let straight = draft(BASE).2;
        let swung = draft(&BASE.replace("swing = 50", "swing = 66")).2;
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
        //
        // Measured as a rate rather than by ear: over these four charts it was twenty-one notes
        // in eight hundred, all of them in the melody, and it is none.
        for (key_text, chart) in [
            ("C minor", "@marusa"),
            ("C major", "@marusa"),
            ("A minor", "@royal-road"),
            ("Eb major", "@naki"),
            ("F# minor", "@junjo"),
        ] {
            let text = format!(
                r#"
                key    = "{key_text}"
                chords = "{chart}"
                form   = "verse chorus"
                seed   = 3
                # Straight, so a note struck on a chord change is heard where it was chosen.
                # This is about which note a part picks, not about when it arrives.
                swing = 50

                [section.verse]
                bars = 8

                [section.chorus]
                bars = 8
                "#
            );
            let (spec, frame, parts) = draft(&text);
            for (part_draft, declared) in parts.iter().zip(&spec.parts) {
                if declared.role.is_drum() {
                    continue;
                }
                for note in &part_draft.notes {
                    let section = &frame.sections[note.section];
                    let Some(event) = section.chord_at(note.start - section.start) else {
                        continue;
                    };
                    let class = PitchClass::new(i32::from(note.pitch));
                    if event.chord.contains(class) {
                        continue;
                    }
                    // A pushed chord belongs to the change it anticipates, not to the chord it
                    // is struck over — judging it against the outgoing chord would flag the very
                    // note the anticipation exists to place.
                    if section
                        .chord_at(note.start + note.length - section.start - Ticks(1))
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
                            "`{}` played {class} a semitone from the {tone} of {} in {key_text} \
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
        // The fixture deliberately contains a diminished triad and a slash chord, which is where
        // a bass line that assumed a perfect fifth above the sounding bass went wrong.
        for chart in [
            "| I | vii | I | V |",
            "@koakuma",
            "@marusa",
            "@junjo",
            "@blues",
        ] {
            let text = format!(
                r#"
                    key = "C major"
                    form = "verse"
                    chords = "{chart}"
                    [section.verse]
                    bars = 4
                    "#
            );
            let (spec, frame, parts) = draft(&text);
            let section = &frame.sections[0];
            for (part_draft, declared) in parts.iter().zip(&spec.parts) {
                if declared.role.is_drum() {
                    continue;
                }
                for note in &part_draft.notes {
                    let class = PitchClass::new(i32::from(note.pitch));
                    let chord = section.chord_at(note.start - section.start);
                    let in_chord = chord.is_some_and(|event| event.chord.contains(class));
                    // A pushed chord anticipates the one sounding where the note ends.
                    let anticipates = section
                        .chord_at(note.start + note.length - section.start - Ticks(1))
                        .is_some_and(|event| event.chord.contains(class));
                    assert!(
                        section.key.scale.contains(section.key.tonic, class)
                            || in_chord
                            || anticipates,
                        "`{}` played {class} over {} in `{chart}`",
                        part_draft.name,
                        chord.map(|e| e.chord.to_string()).unwrap_or_default()
                    );
                }
            }
        }
    }

    #[test]
    fn adding_a_part_leaves_the_other_parts_alone() {
        // Every part hangs off the same skeleton, so taking that skeleton from whichever melody
        // part happened to be in the roster meant adding a part rewrote the whole arrangement.
        let base = r#"
            form = "verse"
            chords = "@axis"
            [section.verse]
            bars = 4
            [[part]]
            name = "bass"
            [[part]]
            name = "kick"
            "#;
        let before = draft(base).2;
        let after = draft(&format!(
            r#"
            {base}
            [[part]]
            name = "extra"
            role = "pad"
            "#
        ))
        .2;
        for name in ["bass", "kick"] {
            assert_eq!(
                part(&before, name).notes,
                part(&after, name).notes,
                "adding a part rewrote `{name}`"
            );
        }
    }

    #[test]
    fn editing_one_section_leaves_the_others_alone() {
        let base = r#"
            form     = "verse chorus"
            chords   = "@axis"
            seed     = 3

            [section.verse]
            bars = 2

            [section.chorus]
            bars      = 2
            intensity = {}
        "#;
        let quiet = draft(&base.replace("{}", "0.9")).2;
        let loud = draft(&base.replace("{}", "0.4")).2;
        for (a, b) in quiet.iter().zip(&loud) {
            let verse_a: Vec<&Draft> = a.notes.iter().filter(|n| n.section == 0).collect();
            let verse_b: Vec<&Draft> = b.notes.iter().filter(|n| n.section == 0).collect();
            assert_eq!(
                verse_a, verse_b,
                "changing the chorus rewrote the verse of `{}`",
                a.name
            );
        }
    }

    #[test]
    fn a_section_can_leave_a_part_out() {
        let (_, _, parts) = draft(
            r#"
            form = "intro chorus"

            [section.intro]
            parts = "bass"
            "#,
        );
        let hat = part(&parts, "hat");
        // Nothing in the intro, which is section zero.
        assert!(
            hat.notes.iter().all(|note| note.section == 1),
            "the hat played in a section it was left out of"
        );
        assert!(!part(&parts, "bass").notes.is_empty());
    }

    #[test]
    fn a_repeated_section_plays_the_same_music() {
        // The section instance used to be part of every stream name, so a second chorus shared
        // nothing with the first and the piece had no chorus, only two sections with one name.
        let (_, frame, parts) = draft(
            r#"
                form = "verse verse"
                chords = "@axis"
                variation = 0
                [section.verse]
                bars = 4
                "#,
        );
        assert_eq!(frame.sections.len(), 3, "two verses and the ending");
        for draft in &parts {
            assert_eq!(
                section_body(&frame, draft, 0),
                section_body(&frame, draft, 1),
                "`{}` played a different second verse",
                draft.name
            );
        }
    }

    #[test]
    fn a_written_rhythm_reaches_every_pitched_part() {
        // The field's contract is that it overrides the generated rhythm, and the format
        // promises that nothing it accepts is quietly ignored. The melody and the drums kept
        // that promise; the chords, the pad, the stab, the arp and the bass read the field,
        // round-tripped it through the document, and played their own rhythm anyway.
        for role in ["chords", "pad", "stab", "arp", "bass"] {
            let text = format!(
                r#"
                form     = "verse"
                chords   = "@axis"
                swing    = 50

                [section.verse]
                bars = 2

                [[part]]
                name   = "{role}"
                rhythm = "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~"
                "#
            );
            let (_, frame, parts) = draft(&text);
            let played = part(&parts, role);
            let notes = section_notes(&frame, played, 0);
            assert!(!notes.is_empty(), "{role} wrote nothing at all");
            let beat = frame.grid.bar_ticks().raw() / 4;
            for (start, ..) in &notes {
                assert!(
                    start % beat == 0,
                    "{role} struck tick {start}, off the written rhythm"
                );
            }
        }
    }

    #[test]
    fn a_subdivision_and_a_gate_reach_only_the_part_that_asked_for_them() {
        // Both live on the part, so turning them up must leave every other part where it was.
        // This is also what makes the fixture in `render` readable: when it moves, the part that
        // moved it is the part that was changed.
        let before = draft(&roster(5, "")).2;
        let after = draft(&roster(5, "subdivision = \"16t\"\n            gate = 0.25")).2;
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
        // fighting the swing and the humanising for the same tick.
        let long = draft(&roster(5, "")).2;
        let short = draft(&roster(5, "gate = 0.25")).2;
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
    fn a_risers_gate_cannot_move_its_note_off_before_the_join() {
        let part = PartSpec {
            gate: 0.25,
            ..PartSpec::of_role("riser", Role::Riser)
        };
        let mut notes = [Draft {
            section: 0,
            pitch: 60,
            velocity: 1.0,
            start: Ticks::ZERO,
            length: Ticks(1_920),
        }];

        shorten(&[part], &mut notes);

        assert_eq!(notes[0].start + notes[0].length, Ticks(1_920));
    }

    #[test]
    fn the_same_spec_writes_the_same_notes_every_time() {
        let first = draft(BASE).2;
        let second = draft(BASE).2;
        for (a, b) in first.iter().zip(&second) {
            assert_eq!(a.notes, b.notes, "`{}` is not deterministic", a.name);
        }
    }

    #[test]
    fn a_different_seed_writes_a_different_piece() {
        let a = draft(&format!(
            r#"
            seed = 1
            {BASE}
            "#
        ))
        .2;
        let b = draft(&format!(
            r#"
            seed = 2
            {BASE}
            "#
        ))
        .2;
        let melody_a = &part(&a, "lead").notes;
        let melody_b = &part(&b, "lead").notes;
        assert_ne!(melody_a, melody_b, "the seed did not reach the melody");
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
        // question the composer is asking the instrument rather than answering — and most of the
        // instruments a composed piece reaches are a SoundFont library or somebody else's plugin,
        // neither of which can be asked. Both of the composer's own timing passes used to write
        // them, the swing on its own and the wander on top of it, and at the default humanisation
        // it was thirteen notes in every hundred.
        //
        // Over the presets rather than a fixture, because two of them are the ones that swing.
        for preset in crate::preset::PRESETS {
            for seed in 0..4u64 {
                let mut spec = preset.spec();
                spec.seed = seed;
                let piece = crate::render::compose(&spec);
                for track in &piece.tracks {
                    for clip in &track.clips {
                        let mut sounding: std::collections::BTreeMap<u8, Ticks> =
                            std::collections::BTreeMap::new();
                        for note in &clip.notes {
                            let Some(ends) = sounding.insert(note.pitch, note.end()) else {
                                continue;
                            };
                            assert!(
                                ends <= note.start,
                                "{} seed {seed}: in `{}` a {} sounding to {} is struck again at {}",
                                preset.name,
                                clip.name,
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
}
