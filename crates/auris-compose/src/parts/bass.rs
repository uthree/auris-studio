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
use crate::theory::chord::Chord;
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
    /// A quarter-note line that reads the chord upward and steps into the next one: the walking
    /// bass. The one figure that does not follow the kick — the walk *is* the timekeeping, which
    /// is the whole reason a trio can play without one.
    Walk,
}

/// The pitches of a walking line: `count` beats over one chord, from its root toward the next.
///
/// A walking bass is a line rather than a series of answers, so it is built whole: beat one
/// states the root, the middle beats read the chord — third then fifth heading up to a chord
/// above, fifth then third heading down to one below, round again where there are beats left —
/// each placed in the octave nearest the note before it, and the last beat steps to one scale
/// note beside the next chord's root. What comes out is the root–third–fifth–approach of every
/// method book one way, and G–F–D–B into C the other, which is the same idea upside down.
///
/// With nowhere to go — the last chord of a piece — the last beat reads the chord like the
/// middle ones, because an approach into nothing is a question nobody answers.
fn walk_line(
    chord: &Chord,
    scale: &ChordScale,
    root: i32,
    target: Option<i32>,
    count: usize,
    low: i32,
    high: i32,
) -> Vec<i32> {
    let classes = chord.classes();
    let direction = target.map_or(1, |next| if next >= root { 1 } else { -1 });
    let mut line = Vec::with_capacity(count);
    let mut previous = root;
    for position in 0..count {
        let pitch = if position == 0 {
            root
        } else if position + 1 == count
            && let Some(next) = target
        {
            approach(scale, previous, next, low, high)
        } else {
            let others = classes.len().saturating_sub(1).max(1);
            let step = (position - 1) % others;
            // Read toward where the line is going: third then fifth on the way up is fifth
            // then third on the way down.
            let index = if direction >= 0 {
                1 + step
            } else {
                classes.len().saturating_sub(1 + step)
            };
            let class = classes.get(index).copied().unwrap_or(chord.root);
            climbing(class, previous, direction, low, high)
        };
        previous = pitch;
        line.push(pitch);
    }
    line
}

/// The octave of `class` nearest `previous`, inside the range. Ties break toward `direction`,
/// which is the way the walking line is heading.
fn climbing(class: PitchClass, previous: i32, direction: i32, low: i32, high: i32) -> i32 {
    let mut best = fold_into(class.midi(2), low, high);
    let mut nearest = i32::MAX;
    for octave in 0..9 {
        let pitch = class.midi(octave);
        if pitch < low || pitch > high {
            continue;
        }
        // Twice the distance, plus one against the line's direction: -d loses the tie to +d
        // heading up, and wins it heading down, and nothing else changes.
        let distance =
            (pitch - previous).abs() * 2 + i32::from((pitch - previous).signum() != direction);
        if distance < nearest {
            nearest = distance;
            best = pitch;
        }
    }
    best
}

/// One scale note beside `target`, for the last beat of a walking line.
///
/// A walking line does not land on the next chord's root — the next chord does that — it stops
/// one step beside it so that the change answers the line. The semitone wins where the scale
/// offers one, which is what puts a leading note under an arriving tonic; at the same distance
/// the side the line comes from wins; and the note the line is already standing on is taken only
/// when nothing else fits, because an approach that does not move is not approaching.
fn approach(scale: &ChordScale, previous: i32, target: i32, low: i32, high: i32) -> i32 {
    let side = if previous > target { 1 } else { -1 };
    let mut candidates = Vec::with_capacity(4);
    for distance in 1..=2 {
        for step in [side, -side] {
            candidates.push(target + step * distance);
        }
    }
    let mut fitting: Vec<i32> = candidates
        .into_iter()
        .filter(|pitch| (low..=high).contains(pitch) && scale.contains(PitchClass::new(*pitch)))
        .collect();
    if fitting.len() > 1 {
        fitting.retain(|pitch| *pitch != previous);
    }
    fitting
        .into_iter()
        .min_by_key(|pitch| (pitch - target).abs())
        .unwrap_or_else(|| fold_into(target, low, high))
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
        const FIGURES: [BassFigure; 5] = [
            BassFigure::Root,
            BassFigure::Fifth,
            BassFigure::Approach,
            BassFigure::Octave,
            BassFigure::Walk,
        ];
        // The same weighting the chords use: sparse reaches for the root alone, busy for the
        // octave line that fills every beat and for the walk.
        let figure = FIGURES[choose
            .weighted(&[
                0.2 + (1.0 - busy) * 2.0,
                1.0,
                0.2 + busy,
                0.2 + busy * 1.6,
                0.1 + busy * 1.3,
            ])
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
                // Every beat and nothing else. The walk does not follow the kick because the
                // walk is the timekeeping: quarter notes whatever the groove is doing, which is
                // what the figure *is*.
                BassFigure::Walk => (0..steps)
                    .filter(|offset| {
                        ((first + offset) % per_bar).is_multiple_of(grid.steps_per_beat().max(1))
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

        // The whole walk at once, because a walking bass is a line rather than a series of
        // answers: each beat is chosen by where the last one was and where the next chord is,
        // and neither is a question one onset can answer for itself.
        let walk = (figure == BassFigure::Walk).then(|| {
            let scale = ChordScale::new(event.key, event.chord);
            walk_line(&event.chord, &scale, root, target, onsets.len(), low, high)
        });

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
                BassFigure::Walk => walk
                    .as_ref()
                    .and_then(|line| line.get(position))
                    .copied()
                    .unwrap_or(root),
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
    fn the_walking_line_reads_the_chord_and_steps_into_the_next() {
        use crate::theory::chord::Quality;
        use crate::theory::key::Key;

        let key = Key::parse("C major").unwrap();
        let (low, high) = crate::spec::Role::Bass.range();

        // Heading up: C to F is the method-book line — root, third, fifth, and the step below
        // the arriving root. E twice is not a repeat: the notes are two beats apart.
        let c = Chord::new(PitchClass::parse("C").unwrap(), Quality::Major);
        let up = walk_line(&c, &ChordScale::new(key, c), 36, Some(41), 4, low, high);
        assert_eq!(up, vec![36, 40, 43, 40]);

        // Heading down: G7 to C reads the chord the other way — G, F, D — and arrives on the
        // leading note, so the tonic lands a semitone above the line that prepared it.
        let g7 = Chord::new(PitchClass::parse("G").unwrap(), Quality::Dominant7);
        let down = walk_line(&g7, &ChordScale::new(key, g7), 43, Some(36), 4, low, high);
        assert_eq!(down, vec![43, 41, 38, 35]);

        // Every note of both belongs to the chord or the key, which is the crate's own rule.
        for line in [&up, &down] {
            for pitch in line {
                let class = PitchClass::new(*pitch);
                assert!(
                    key.scale.contains(key.tonic, class) || c.contains(class) || g7.contains(class),
                    "{pitch} is outside the harmony"
                );
            }
        }

        // Three beats is the same idea one note shorter, and the last chord of a piece has
        // nothing to approach, so it reads the chord to the end instead.
        assert_eq!(
            walk_line(&c, &ChordScale::new(key, c), 36, Some(41), 3, low, high),
            vec![36, 40, 43],
            "root, third, and an approach that will not stand on the note it just played"
        );
        let nowhere = walk_line(&c, &ChordScale::new(key, c), 36, None, 4, low, high);
        assert_eq!(
            nowhere,
            vec![36, 40, 43, 40],
            "third again, not an approach"
        );
    }

    #[test]
    fn a_busy_bass_can_walk() {
        // The figure has to be reachable: some bar of some seed walks quarter notes through
        // the chord's third, which no other figure plays — the rest are root, fifth and octave.
        let mut walked = false;
        for seed in 1..=16u64 {
            let (_, frame, parts) = draft(&format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    seed = {seed}
                    [section.verse]
                    bars = 4
                    [[part]]
                    name = "bass"
                    density = 1.0
                    "#
            ));
            let bass = part(&parts, "bass");
            let section = &frame.sections[0];
            for event in &section.events {
                let third = event.chord.classes().get(1).copied();
                let in_event: Vec<&Draft> = bass
                    .notes
                    .iter()
                    .filter(|note| {
                        note.start >= section.start + event.start
                            && note.start < section.start + event.end()
                    })
                    .collect();
                if third.is_some_and(|third| {
                    in_event
                        .iter()
                        .any(|note| PitchClass::new(i32::from(note.pitch)) == third)
                }) {
                    walked = true;
                }
            }
        }
        assert!(walked, "no bar in sixteen seeds walked through a third");
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
