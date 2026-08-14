//! The bass line.
//!
//! Locked to the kick *pattern* rather than to the kick *part*: reading the groove keeps the two
//! together without making one part depend on another's notes, which is the rule the whole module
//! is built on. That is also why the arithmetic that reads a groove on the drums' grid rather than
//! the bass's own lives here and not with the kit — it is the bass following, not the drummer
//! leading.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::DrumVoice;
use crate::spec::PartSpec;
use crate::theory::chord_scale::ChordScale;
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

use super::writer::{bar_stream, density, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// The octave a bass leaps to from `root`, staying inside `low..=high`.
///
/// Up where there is room and down where there is not, which is what a player does: the octave
/// above a high root is off the end of the instrument and the one below is right there.
///
/// It used to fold `root + OCTAVE` back into the range, and the range is exactly two octaves wide
/// with the roster's roots sitting in the upper one — so for every root from F upward the octave
/// above fell outside, `fold_into` subtracted it straight back, and the answer was the root
/// itself. The figure silently stopped being an octave for four of the seven diatonic degrees,
/// the subdominant and the dominant among them: the bass restruck the note it was already on
/// while the recipe said it was leaping.
fn octave_leap(root: i32, low: i32, high: i32) -> i32 {
    if root + OCTAVE <= high {
        root + OCTAVE
    } else if root - OCTAVE >= low {
        root - OCTAVE
    } else {
        // A range narrower than an octave holds no leap at all. Restriking the root is the honest
        // answer, and the only one left.
        root
    }
}

/// The shape of a bass line through a bar.
///
/// Same reason as [`CompFigure`](super::comp::CompFigure): the bass followed the kick and
/// alternated root and fifth, which is one bass line and not a choice of them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BassFigure {
    /// The root, and nothing else. Solid, and what most of rock does.
    Root,
    /// Root on the strong hits, fifth on the weak ones: the oldest bass line there is.
    Fifth,
    /// The root, jumping an octave on the weak hits.
    Octave,
    /// Root and fifth, stepping into the next chord on the last hit before it.
    Approach,
}

/// The bass line.
///
/// Locked to the kick pattern rather than to the kick *part*: reading the groove keeps the two
/// together without making one part depend on another's notes.
pub(super) fn bass(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = part_grid(frame, part);
    let kick = crate::frame::groove_pattern(&settings.groove, DrumVoice::Kick);
    // Asked in ticks and answered on the *drums'* grid, not the bass's. A groove is read by index,
    // so a bass dividing its beats any other way would have wrapped the pattern partway through
    // the bar and followed a kick nobody was playing.
    //
    // Mapped onto the bar the same way the drummer maps it, too. Reading the raw index instead
    // wrapped a groove shorter than the bar and truncated one longer, so in every meter the
    // groove was not written for the bass followed a kick the kit was not striking.
    let drums = frame.grid;
    let drum_bar = drums.bar_ticks().raw().max(1);
    let own = crate::frame::groove_steps_per_beat(&settings.groove);
    let kick_at = |at: Ticks| {
        let step = drums.step_of(Ticks(at.raw().rem_euclid(drum_bar)));
        kick.at_in_bar(step, drums.steps_per_bar(), drums.steps_per_beat(), own)
            .is_some()
    };
    let mut notes = Vec::new();

    for (position_in_section, event) in section.events.iter().enumerate() {
        let root = fold_into(event.chord.bass_class().midi(part.octave), low, high);
        // The chord's own fifth, read off the chord rather than assumed perfect and measured
        // from the chord's root rather than from a slash bass. A blind `root + 7` played F# over
        // a B diminished and a C over a G/F — notes in neither the chord nor the key.
        let fifth_class = event
            .chord
            .classes()
            .get(2)
            .copied()
            .unwrap_or(event.chord.root);
        let fifth = fold_into(fifth_class.midi(part.octave), low, high);

        let steps = grid.step_of(event.length).max(1);
        // The groove is written as one bar, so it is read modulo the bar rather than modulo its
        // own length — otherwise a meter that is not sixteen steps drifts against the drums.
        let per_bar = grid.steps_per_bar().max(1);
        let first = grid.step_of(event.start) % per_bar;
        // Which line to play over this chord, drawn from the section's own stream so a repeat
        // plays the same line and a different seed plays a different one.
        let bar = grid.step_of(event.start) / grid.steps_per_bar().max(1);
        let busy = density(settings, part, section);
        let mut choose = bar_stream(settings, frame, part, section, "figure", bar);
        const FIGURES: [BassFigure; 4] = [
            BassFigure::Root,
            BassFigure::Fifth,
            BassFigure::Approach,
            BassFigure::Octave,
        ];
        // The same weighting the chords use: sparse reaches for the root alone, busy for the
        // octave line that fills every beat.
        let figure = FIGURES[choose
            .weighted(&[0.2 + (1.0 - busy) * 2.0, 1.0, 0.2 + busy, 0.2 + busy * 1.6])
            .min(FIGURES.len() - 1)];

        // The figure decides how busy the line is as well as what it plays. Two lines that hit
        // the same beats and differ only on the weak ones are the same line to a listener.
        // A rhythm the user wrote replaces the figure's rhythm outright — the figure still
        // chooses the notes, which is the half of the job the field never claimed.
        let mut onsets: Vec<usize> = match part.rhythm.as_ref() {
            Some(pattern) => (0..steps)
                .filter(|offset| pattern.at((first + offset) % per_bar).is_some())
                .collect(),
            None => match figure {
                // One note under the chord, held: the sound of a bass player staying out of
                // the way.
                BassFigure::Root => Vec::new(),
                // Follow the kick, which is what locks a rhythm section together.
                BassFigure::Fifth | BassFigure::Approach => (0..steps)
                    .filter(|offset| kick_at(event.start + grid.tick_of(*offset)))
                    .collect(),
                // The kick, and the half-beats between it: a busier, walking feel.
                BassFigure::Octave => (0..steps)
                    .filter(|offset| {
                        kick_at(event.start + grid.tick_of(*offset))
                            || ((first + offset) % per_bar)
                                .is_multiple_of(grid.steps_per_beat().max(1))
                    })
                    .collect(),
            },
        };
        // Always sound the chord's start, so a change of chord is heard whatever the figure —
        // except under a written rhythm, whose rests are part of the instruction.
        if part.rhythm.is_none() && !onsets.contains(&0) {
            onsets.insert(0, 0);
        }

        // Where the next chord's root is, for a line that steps into it.
        let target = section
            .events
            .get(position_in_section + 1)
            .map(|next| fold_into(next.chord.bass_class().midi(part.octave), low, high));

        let last = onsets.len().saturating_sub(1);
        for (position, offset) in onsets.iter().enumerate() {
            let at = event.start + grid.tick_of(*offset);
            let next = onsets
                .get(position + 1)
                .map(|next| grid.tick_of(*next))
                .unwrap_or(event.length);
            let length = (next - grid.tick_of(*offset)).max(grid.step_ticks());
            let strong = position == 0 || grid.weight(grid.step_of(at)) >= 2;
            let pitch = match figure {
                BassFigure::Root => root,
                BassFigure::Fifth => {
                    if strong {
                        root
                    } else {
                        fifth
                    }
                }
                BassFigure::Octave => {
                    if strong {
                        root
                    } else {
                        octave_leap(root, low, high)
                    }
                }
                // Stepping into whatever comes next, on the last hit before it. A bass player
                // reaching for the next chord is the sound of a line going somewhere, and it is
                // the one figure here that needs to know what the next chord is.
                //
                // From inside the key, not a semitone below. A chromatic approach is what a jazz
                // player would reach for, but every other note this crate writes belongs to the
                // chord or to the key, and one part quietly breaking that would be a wrong note
                // to anybody reading the piano roll. Inside the scale the chord implies, so an
                // approach into a dominant does not walk through the degree it just altered.
                BassFigure::Approach if position == last && last > 0 => target
                    .and_then(|next| {
                        let scale = ChordScale::new(event.key, event.chord);
                        (1..=3)
                            .map(|step| next - step)
                            .find(|pitch| scale.contains(PitchClass::new(*pitch)))
                            .map(|pitch| fold_into(pitch, low, high))
                    })
                    .unwrap_or(fifth),
                BassFigure::Approach => {
                    if strong {
                        root
                    } else {
                        fifth
                    }
                }
            };
            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(
                    grid.weight(grid.step_of(at)),
                    section.intensity,
                    settings.dynamics,
                ) * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.05, 1.0),
                start: section.start + at,
                // Floored at a tick, not at a step. A step is often *longer* than the room left
                // in the chord — nine chords in a four-four bar leave 426 ticks and an eighth-note
                // grid asks for 480 — and flooring there put the note straight back over the
                // boundary the `min` had just brought it inside, so two chords' bass notes sounded
                // on top of each other. Every other writer in this crate floors at `Ticks(1)`,
                // which can never exceed a `min` that is itself a real length.
                length: length.min(event.end() - at).max(Ticks(1)),
            });
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::fixture::{BASE, draft, part};

    #[test]
    fn the_octave_figure_always_moves_an_octave() {
        // The range a bass actually writes in, which is exactly two octaves wide.
        let (low, high) = crate::spec::Role::Bass.range();
        assert_eq!(
            high - low,
            2 * OCTAVE,
            "the range this test is about changed"
        );

        // Every root the roster can sit on — `Role::Bass` puts them in octave 2, so 36..=47.
        for root in 36..=47 {
            let leapt = octave_leap(root, low, high);
            assert_eq!(
                (leapt - root).abs(),
                OCTAVE,
                "the root {root} did not move an octave"
            );
            assert!(
                (low..=high).contains(&leapt),
                "the root {root} leapt out of the range"
            );
        }

        // Which way it goes: up while there is room above, down once there is not. Both are an
        // octave and both are what a player would reach for.
        assert_eq!(octave_leap(36, low, high), 48);
        assert_eq!(octave_leap(40, low, high), 52);
        assert_eq!(
            octave_leap(41, low, high),
            29,
            "F used to answer with itself"
        );
        assert_eq!(
            octave_leap(47, low, high),
            35,
            "B used to answer with itself"
        );

        // A window with no octave in it has no leap, and says so rather than pretending.
        assert_eq!(octave_leap(40, 36, 44), 40);
    }

    #[test]
    fn the_bass_sounds_every_chord_change() {
        let (_, frame, parts) = draft(BASE);
        let bass = part(&parts, "bass");
        let section = &frame.sections[0];
        for event in &section.events {
            let at = section.start + event.start;
            assert!(
                bass.notes.iter().any(|note| note.start == at),
                "the bass missed the change at {}",
                event.start.raw()
            );
        }
    }

    #[test]
    fn the_bass_plays_the_sounding_bass_of_a_slash_chord() {
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            chords = "@koakuma"
            humanize = 0
            [section.verse]
            bars = 4
            "#,
        );
        let bass = part(&parts, "bass");
        let section = &frame.sections[0];
        // Bar two is V over the subdominant, so the bass must play the subdominant.
        let event = &section.events[1];
        let expected = event.chord.bass_class();
        let note = bass
            .notes
            .iter()
            .find(|note| note.start == section.start + event.start)
            .expect("a note at the change");
        assert_eq!(PitchClass::new(i32::from(note.pitch)), expected);
    }

    #[test]
    fn no_bass_note_outlives_the_chord_it_belongs_to() {
        // More chords in a bar than the grid has steps, which the chart syntax allows and a dial
        // can ask for: nine chords in four four leave 426 ticks each, and an eighth-note step is
        // 480. Flooring the length at a whole step re-inflated every note past its chord, so eight
        // of the nine overlapped the next by 54 ticks — two roots sounding at once on one
        // instrument, which is the one thing a bass part must not do.
        let (_, frame, parts) = draft(
            r#"
                form = "verse"
                chords = "| I ii iii IV V vi vii I bII |"
                humanize = 0
                [section.verse]
                bars = 1
                [section.verse.part.bass]
                subdivision = "8"
                "#,
        );
        let bass = part(&parts, "bass");
        let section = &frame.sections[0];
        assert!(bass.notes.len() >= 9, "one note per chord at least");

        for event in &section.events {
            let from = section.start + event.start;
            let to = section.start + event.end();
            for note in bass
                .notes
                .iter()
                .filter(|note| note.start >= from && note.start < to)
            {
                assert!(
                    note.start + note.length <= to,
                    "a note at {} ran {} ticks past its chord's end at {}",
                    note.start.raw(),
                    (note.start + note.length - to).raw(),
                    to.raw()
                );
            }
        }

        // And the general form of it: no two bass notes overlap at all.
        let mut sorted = bass.notes.clone();
        sorted.sort_by_key(|note| note.start);
        for pair in sorted.windows(2) {
            assert!(
                pair[0].start + pair[0].length <= pair[1].start,
                "the note at {} overlaps the one at {}",
                pair[0].start.raw(),
                pair[1].start.raw()
            );
        }
    }

    #[test]
    fn the_bass_follows_the_kick_in_an_odd_meter() {
        // The groove is a bar long, so it has to be read modulo the bar rather than modulo its
        // own sixteen steps, or it drifts against the drums in anything but four four.
        let (_, frame, parts) = draft(
            r#"
                form = "verse"
                meter = "3/4"
                chords = "@axis"
                humanize = 0
                [section.verse]
                bars = 4
                "#,
        );
        let bass = part(&parts, "bass");
        let kick = part(&parts, "kick");
        assert!(!bass.notes.is_empty() && !kick.notes.is_empty());
        // Every kick that survived thinning should have a bass note with it somewhere in the bar.
        let bar = frame.grid.bar_ticks();
        for note in &kick.notes {
            let bar_index = note.start.raw() / bar.raw();
            assert!(
                bass.notes
                    .iter()
                    .any(|other| other.start.raw() / bar.raw() == bar_index),
                "no bass in the bar with a kick at {}",
                note.start.raw()
            );
        }
    }
}
