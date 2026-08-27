//! The tune.
//!
//! One figure per part and section, restated bar after bar and turned over where a phrase turns
//! over, which is what leaves a section with something an ear can hold on to. The motif, the way
//! it is varied and the two scale walks it is stated through are all here because nothing else in
//! the band writes one: this is the only part whose notes are a shape rather than a harmony.
//!
//! That is also why it is the hard part. Every other writer is told what to play by the chord
//! under it; this one is only told where to hang. What the rules below are, where their numbers
//! come from and what the line measured like before they existed is [`crate::melodic`] — read it
//! first if any constant here looks arbitrary, because every one of them was argued there.

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, Grid, Pattern};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::PartSpec;
use crate::theory::chord_scale::ChordScale;
use crate::theory::pitch::{OCTAVE, PitchClass, fold_into};

use super::writer::{bar_onsets, bar_stream, density, dynamic, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// Fewest notes a generated figure is allowed to have.
///
/// Three is the smallest number that can carry a shape: two notes are an interval, and one is a
/// note. It is also the smallest [`vary_motif`] has anything to work with.
const MOTIF_MINIMUM: usize = 3;

/// The moves a figure may make, in scale steps, and how often each is drawn.
///
/// A scale step is a second, two is a third, three is a fourth — so this table *is* the melodic
/// interval distribution, and it was measured against the corpus figures rather than guessed at.
/// See [`crate::melodic`] for where the numbers come from and what the figure used to draw from,
/// which was half seconds and a quarter *fourths*: an arpeggio's distribution rather than a
/// tune's.
///
/// Zero is here because a repeated note is a melodic move like any other and one in nine of them
/// is one. Without it every note had to go somewhere, and the repeats that did occur were
/// accidents of the range clamp rather than anything anybody wrote.
const MOVES: [i32; 7] = [-3, -2, -1, 0, 1, 2, 3];

/// How often each of [`MOVES`] is drawn.
const MOVE_WEIGHTS: [f32; 7] = [3.0, 7.5, 34.0, 11.0, 34.0, 7.5, 3.0];

/// How often a figure keeps the move it drew rather than being turned by [`shaped`].
///
/// Neither of the two regularities is a law — a melody that filled in every one of its leaps
/// would be as mechanical as one that never did — and this is the room left for the exceptions.
/// Huron reports post-leap reversal at about seven times in ten, which is what this lands near
/// once the drawn direction is taken into account.
const HOLD: f32 = 0.3;

/// The move a figure makes next, given the one it just made.
///
/// Two regularities that every corpus of tonal melody shows, and that a memoryless walk cannot
/// have: after a leap the line turns and fills the gap in, and after a step it tends to carry on
/// the same way. `drawn` is what the table gave; `hold` is the caller's roll for leaving it alone.
///
/// A drawn *leap* is never turned by the inertia rule, only a drawn step. That is what keeps the
/// line from running to the end of its range: with every step redirected and no leap free to
/// answer, a figure that started upward would only ever go up.
fn shaped(previous: i32, drawn: i32, hold: bool) -> i32 {
    if drawn == 0 || previous == 0 || hold {
        return drawn;
    }
    if previous.abs() >= 2 {
        // Gap-fill: a leap implies a turn, and what fills a gap is a step rather than a leap back.
        -previous.signum() * drawn.abs().min(2)
    } else if drawn.abs() == 1 {
        // Step inertia.
        previous.signum()
    } else {
        drawn
    }
}

/// How far a figure may wander from its anchor before it is turned back.
///
/// Four scale steps is a sixth. The hard clamp is at six, and the point of turning earlier is
/// that the walk never *reaches* the clamp: a walk held against a wall repeats the note it is
/// standing on.
const WANDER: i32 = 4;

/// `move_by` turned back toward the anchor when the figure has wandered too far from it.
///
/// Melodic regression to the mean: pitches at the extremes of a melody's range are followed by
/// pitches nearer the middle. It is also, on von Hippel and Huron's account, where most of
/// post-leap reversal actually comes from — the gap-fill rule seen from one side.
///
/// It earns its place here for a blunter reason. Step inertia gives the walk a memory and so a
/// direction, and a walk with a direction and a clamp ends up against the clamp: adding inertia
/// on its own took the composer from six repeated notes in every hundred intervals to
/// thirty-one, because a figure that reached its ceiling stood there until something turned it
/// round. This is what turns it round.
fn toward_middle(degree: i32, move_by: i32) -> i32 {
    if degree.abs() >= WANDER && degree.signum() == move_by.signum() {
        -move_by
    } else {
        move_by
    }
}

/// Which offset a restated figure takes, so that it carries on from where the last bar left off.
///
/// `landings` are the pitches the figure's first note would sound at, one per offset the caller
/// is willing to shift the whole figure by. The smoothest join wins; ties go to the smallest
/// shift, which keeps the figure near the structural pitch it was written against.
///
/// Without this the figure restarted from its anchor every bar, and the join between one bar and
/// the next was whatever fell out of the difference between two anchors — a leap of a fourth on
/// average, nearly half of them wider than that, and not one of them chosen by anything.
///
/// Anything up to a *fourth* is preferred to landing on the note the last bar ended on. Restating
/// a figure from the note it just finished on is a real melodic move and not one to make a third
/// of the time, which is what taking the nearest landing outright did: the smoothest join
/// available is always the one that does not move. The rank was 3 — a repeat lost only to a step
/// — and a fifth of all bar crossings still restated the last note, because the landings are
/// chord-snapped now and the nearest *other* chord tone is usually a third or a fourth away. At 5
/// those win too, and what is left repeating is the join whose every alternative is a real leap,
/// which is the one a repeat genuinely beats. Measured at 3, 4 and 5 over the presets: the mean
/// join and the leap share did not move, and only the repeats did.
fn join_offset(previous: i32, landings: &[(i32, i32)]) -> i32 {
    landings
        .iter()
        .min_by_key(|(offset, pitch)| {
            let distance = (pitch - previous).abs();
            let ranked = if distance == 0 { 5 } else { distance };
            (ranked, offset.abs())
        })
        .map(|(offset, _)| *offset)
        .unwrap_or(0)
}

/// How long the last note of a closing phrase sounds for, so that the phrase is let go of.
///
/// A beat of silence before the bar line, or the length it already had if that was shorter — a
/// note is never *lengthened* here, because a figure that ends early ends early on purpose.
/// Never shorter than one step, because a phrase ending in nothing at all is not an ending.
///
/// A beat and not two: this is a breath between phrases and not a rest between sections.
fn breath(grid: Grid, step: usize, length: usize) -> usize {
    let beat = grid.steps_per_beat().max(1);
    let room = grid.steps_per_bar().saturating_sub(step + beat);
    length.min(room).max(1)
}

/// How far a restated figure may be shifted to make its join, in scale steps.
///
/// Two is a third, which is enough to close almost any join and little enough that the figure is
/// still sitting on the structural pitch the skeleton chose for it. Wider, and the arch the
/// skeleton draws over the section stops being what the tune follows.
const JOIN_REACH: i32 = 2;

/// A short figure the melody is built out of.
///
/// Written in scale steps from whatever pitch the frame's skeleton puts under it rather than in
/// absolute notes, so restating it over a different chord keeps its shape while still belonging
/// to the harmony.
#[derive(Clone, Debug)]
struct Motif {
    cells: Vec<Cell>,
}

/// One note of a [`Motif`].
#[derive(Copy, Clone, Debug)]
struct Cell {
    /// Step of the bar it starts on.
    step: usize,
    accent: Accent,
    /// Steps it sounds for. Fewer than the gap to the next cell leaves a rest.
    length: usize,
    /// Scale steps above or below the bar's anchor pitch.
    degree: i32,
}

/// Invents the figure a section is built from.
///
/// Drawn once per part and section and then restated, which is what gives a section something an
/// ear can hold on to. A part with a written rhythm gets that rhythm; only the shape is invented.
fn motif(
    grid: Grid,
    pattern: Option<&Pattern>,
    density: f32,
    syncopation: f32,
    rng: &mut Rng,
) -> Motif {
    let steps = grid.steps_per_bar();
    let mut onsets = bar_onsets(grid, pattern, density, syncopation, rng);
    // A figure needs a few notes to be one, and one note cannot be varied at all. A thin roll
    // used to average out because every bar rolled again; now a single roll decides the whole
    // section, so a thin one would leave the section with nothing rather than with a quiet bar.
    // The strongest free steps are filled first, which is where a note would have gone anyway.
    if pattern.is_none() && onsets.len() < MOTIF_MINIMUM {
        let mut spare: Vec<usize> = (0..steps)
            .filter(|step| !onsets.iter().any(|(taken, _)| taken == step))
            .collect();
        spare.sort_by_key(|step| std::cmp::Reverse(grid.weight(*step)));
        for step in spare.into_iter().take(MOTIF_MINIMUM - onsets.len()) {
            onsets.push((step, Accent::Normal));
        }
        onsets.sort_by_key(|(step, _)| *step);
    }

    let mut cells = Vec::with_capacity(onsets.len());
    let mut degree = 0i32;
    let mut previous_move = 0i32;

    for (position, (step, accent)) in onsets.iter().enumerate() {
        // Mostly steps with the occasional leap, and bounded either side of the anchor: a figure
        // that wandered off would not be recognisable when it came back.
        if position > 0 {
            let drawn = MOVES[rng.weighted(&MOVE_WEIGHTS)];
            let move_by = toward_middle(degree, shaped(previous_move, drawn, rng.chance(HOLD)));
            let landed = (degree + move_by).clamp(-6, 6);
            // What the figure *did*, not what it was asked to do. At the edge of the clamp the
            // two differ, and remembering the request would have the next note fill in a leap
            // that never happened.
            previous_move = landed - degree;
            degree = landed;
        }
        let next = onsets
            .get(position + 1)
            .map(|(next, _)| *next)
            .unwrap_or(steps);
        let gap = next.saturating_sub(*step).max(1);
        // The figure's last note stops short of the next bar, which is where the rest that lets a
        // phrase breathe comes from. Inside the figure a note is occasionally detached too.
        let length = if position + 1 == onsets.len() || rng.chance(0.25) {
            1 + rng.below(gap)
        } else {
            gap
        };
        cells.push(Cell {
            step: *step,
            accent: *accent,
            length: length.clamp(1, gap),
            degree,
        });
    }
    Motif { cells }
}

/// The figure with one thing about it changed.
///
/// Enough to stop four bars of the same bar, not so much that it stops being the same figure —
/// which is the difference between a variation and a different tune.
fn vary_motif(figure: &Motif, rng: &mut Rng) -> Motif {
    let mut cells = figure.cells.clone();
    if cells.len() < 2 {
        return Motif { cells };
    }
    match rng.below(3) {
        // Move the last note somewhere else, which is what turns a statement into a question.
        0 => {
            let last = cells.len() - 1;
            cells[last].degree += if rng.chance(0.5) { 2 } else { -2 };
        }
        // Take a note out, leaving a hole where the ear expects one.
        1 if cells.len() > 2 => {
            let doomed = 1 + rng.below(cells.len() - 1);
            cells.remove(doomed);
        }
        // Turn the figure over from its second note on.
        _ => {
            for cell in cells.iter_mut().skip(1) {
                cell.degree = -cell.degree;
            }
        }
    }
    Motif { cells }
}

/// The tune.
pub(super) fn melody(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let grid = part_grid(frame, part);
    let (low, high) = part.range();
    let density = density(settings, part, section);

    // One figure per part and section, restated bar after bar. Keyed by neither the bar nor the
    // instance, so every bar of every playing reaches for the same one.
    let mut invent = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("motif"),
            RngKey::Word(&section.name),
        ],
    );
    let figure = motif(
        grid,
        part.rhythm.as_ref(),
        density,
        settings.mood.syncopation,
        &mut invent,
    );

    let mut notes = Vec::new();
    // The pitch the last bar finished on, which the next one is joined to. `None` until the
    // section has sounded a note, where the figure simply starts where its anchor puts it.
    let mut left_off: Option<i32> = None;
    for bar in 0..section.bars {
        let mut rng = bar_stream(settings, frame, part, section, "melody", bar);
        // Four bars is the phrase almost everything is built in: state the figure, restate it,
        // and then answer it. The fourth bar is where a tune stops repeating and goes somewhere.
        let closing = bar % 4 == 3;
        let cells = if closing || rng.chance(0.15) {
            vary_motif(&figure, &mut rng)
        } else {
            figure.clone()
        };
        let bar_start = grid.bar_ticks() * bar as i64;

        // Where the whole figure sits this bar. The shape is the figure's; the height is whatever
        // joins it to the bar before, within `JOIN_REACH` of the structural pitch.
        let offset = match (left_off, cells.cells.first()) {
            (Some(previous), Some(first)) => {
                let at = bar_start + grid.tick_of(first.step);
                match section.chord_at(at) {
                    Some(event) => {
                        let scale = ChordScale::new(event.key, event.chord);
                        let anchor = section
                            .skeleton
                            .get(section.event_index_at(at))
                            .copied()
                            .unwrap_or((low + high) / 2);
                        // The landing as it will actually sound. A bar's first note is almost
                        // always on a strong step, where the loop below snaps the pitch onto a
                        // chord tone *after* the join has been chosen — so a join chosen against
                        // the raw pitch was optimising a note that never played, and the snap
                        // pulled a fifth of all bar crossings back onto the very note the last
                        // bar ended on.
                        let snapped = grid.weight(first.step) >= 3;
                        let landings: Vec<(i32, i32)> = (-JOIN_REACH..=JOIN_REACH)
                            .map(|offset| {
                                let pitch =
                                    shift_within(&scale, anchor, first.degree + offset, low, high);
                                let pitch = if snapped {
                                    fold_into(event.chord.nearest_tone(pitch), low, high)
                                } else {
                                    pitch
                                };
                                (offset, pitch)
                            })
                            .collect();
                        join_offset(previous, &landings)
                    }
                    None => 0,
                }
            }
            _ => 0,
        };

        // The bar's pitches, before the harmony has its say over them.
        let mut bar_notes: Vec<(usize, i32, i32)> = Vec::with_capacity(cells.cells.len());
        for (position, cell) in cells.cells.iter().enumerate() {
            let at = bar_start + grid.tick_of(cell.step);
            let Some(event) = section.chord_at(at) else {
                continue;
            };
            let event_index = section.event_index_at(at);
            let weight = grid.weight(cell.step);
            let anchor = section
                .skeleton
                .get(event_index)
                .copied()
                .unwrap_or((low + high) / 2);

            // The figure is written in scale steps from the chord's structural pitch, so it keeps
            // its shape while the harmony moves under it — in the scale of the *event's* key,
            // so a modulation inside the range moves the scale at the tick it moves the chords,
            // and in the scale the *chord* implies there, so a borrowed note is not answered by
            // the degree it borrowed from.
            let scale = ChordScale::new(event.key, event.chord);
            let mut pitch = shift_within(&scale, anchor, cell.degree + offset, low, high);
            // A note on a strong step has to agree with the chord, or the figure's shape wins an
            // argument with the harmony that it should not be having. The last note of a closing
            // bar is treated as strong however weak its step: a phrase that ends on a passing note
            // has not ended, it has stopped.
            let cadence = closing && position + 1 == cells.cells.len();
            if weight >= 3 || cadence {
                pitch = fold_into(event.chord.nearest_tone(pitch), low, high);
            } else if let Some(&(previous_position, previous_pitch, _)) = bar_notes.last() {
                // Two cells that asked for different degrees and arrived on one pitch: the range
                // has folded the walk flat, and the repeat is nobody's. See `unstick`.
                let asked = cell.degree - cells.cells[previous_position].degree;
                if pitch == previous_pitch && asked != 0 {
                    pitch = unstick(&scale, pitch, asked.signum(), low, high);
                }
            }
            bar_notes.push((position, pitch, i32::from(weight)));
        }

        // A dissonance that is left by a leap is not heard as a dissonance leaning on its
        // neighbour; it is heard as a wrong note. Either the note beside it is a step away or it
        // has to be a note of the chord — and the figure's rhythm is what the ear is following, so
        // the pitch is what gives way.
        for position in 0..bar_notes.len() {
            let (_, pitch, _) = bar_notes[position];
            let Some(event) =
                section.chord_at(bar_start + grid.tick_of(cells.cells[bar_notes[position].0].step))
            else {
                continue;
            };
            if event.chord.contains_midi(pitch) {
                continue;
            }
            let leaves_by_leap = match bar_notes.get(position + 1) {
                Some((_, next, _)) => (next - pitch).abs() > 2,
                // The last note of the bar leaves into the next bar's first, and `join_offset`
                // has already put that within a step or two — so it resolves, and pulling it onto
                // a chord tone here only produced a note the next bar then repeated.
                None => false,
            };
            if !leaves_by_leap {
                continue;
            }
            let resolved = fold_into(event.chord.nearest_tone(pitch), low, high);
            // Unless resolving it would make the figure stutter. A dissonance left by a leap is
            // one wrong-sounding note; the same note struck twice in a row where the figure asked
            // for two different ones is a fault the ear hears as the *rhythm* misfiring, which is
            // worse. Measured: this rule on its own put three repeats in every hundred intervals.
            let neighbours = [
                position
                    .checked_sub(1)
                    .and_then(|before| bar_notes.get(before)),
                bar_notes.get(position + 1),
            ];
            if neighbours
                .iter()
                .flatten()
                .any(|(_, beside, _)| *beside == resolved)
            {
                continue;
            }
            bar_notes[position].1 = resolved;
        }

        for (index_in_bar, (position, pitch, weight)) in bar_notes.iter().enumerate() {
            let cell = &cells.cells[*position];
            let at = bar_start + grid.tick_of(cell.step);
            let weight = *weight as u8;
            let last_in_bar = index_in_bar + 1 == bar_notes.len();
            if last_in_bar {
                left_off = Some(*pitch);
            }
            let pitch = *pitch;
            // A phrase has to be let go of. The figure already stops somewhere short of the bar
            // line, but by as little as one step — and how much air it leaves was a draw rather
            // than a decision, so about half of the composer's phrases ran on into the next one
            // with nothing between them. At the end of a four-bar phrase it is a decision: the
            // last note ends a beat before the bar does, or sooner if it was already shorter.
            let length = if closing && last_in_bar {
                breath(grid, cell.step, cell.length)
            } else {
                cell.length
            };

            notes.push(Draft {
                section: index,
                pitch: pitch.clamp(0, 127) as u8,
                velocity: (velocity(weight, section.intensity, settings.dynamics)
                    * dynamic(cell.accent.scale(), settings.dynamics)
                    * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.05, 1.0),
                start: section.start + at,
                length: grid.step_ticks() * length.max(1) as i64,
            });
        }
    }
    notes
}

/// The pitch `steps` scale degrees from `from`.
///
/// The scale is a parameter rather than the section's, for two reasons. A section written over a
/// document can modulate inside itself, so the caller passes the key of the chord the note sounds
/// over; and a chord that borrows a note replaces the degree it borrowed from, so what the caller
/// passes is [`ChordScale`] rather than the key's own seven notes.
fn scale_shift(scale: &ChordScale, from: i32, steps: i32) -> i32 {
    let tonic = scale.tonic();
    let semitones = tonic.distance_up_to(PitchClass::new(from));
    let octaves = (from - tonic.midi(0) - semitones) / OCTAVE;
    let degree = scale.nearest_degree(semitones) + octaves * scale.degree_count() as i32;
    tonic.midi(0) + scale.semitone(degree + steps)
}

/// `pitch` moved one scale step off the note before it, for when the range has folded two
/// different degrees onto one pitch.
///
/// Near the edge of its range [`shift_within`] shrinks an interval rather than folding it, and
/// two cells that asked for *different* degrees can arrive on the same note — a repeat nobody
/// drew, at the top of the figure's arch, which is exactly where the line is most audible.
/// Measured over the presets, one repeat in eight sat within two semitones of a range edge.
///
/// The move goes the way the figure was going; at the very edge, where that direction has no
/// room, it turns back instead, which is what regression to the mean would have done a note
/// later anyway. A repeat the figure asked for — two cells on one degree — is not this
/// function's business and never reaches it.
fn unstick(scale: &ChordScale, pitch: i32, direction: i32, low: i32, high: i32) -> i32 {
    for step in [direction, -direction] {
        let moved = scale_shift(scale, pitch, step);
        if (low..=high).contains(&moved) {
            return moved;
        }
    }
    pitch
}

/// `anchor` shifted by `degree` scale steps, kept inside `low..=high` by shrinking the interval.
///
/// Folding an out-of-range note back by octaves moves it twelve semitones, which is a wider leap
/// than any the figure asked for — so a shape chosen to be smooth arrived with a jump in it that
/// nothing had priced. Pulling the interval in instead keeps the direction the figure was going,
/// which is what an ear follows. Folding is kept only for the case where even the anchor is out
/// of range, where there is nothing left to shrink.
fn shift_within(scale: &ChordScale, anchor: i32, degree: i32, low: i32, high: i32) -> i32 {
    let mut steps = degree;
    loop {
        let pitch = scale_shift(scale, anchor, steps);
        if (low..=high).contains(&pitch) {
            return pitch;
        }
        if steps == 0 {
            return fold_into(pitch, low, high);
        }
        steps -= steps.signum();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::fixture::{BASE, bar_steps, draft, part};
    use crate::spec::Role;
    use auris_core::time::Ticks;

    #[test]
    fn the_melody_restates_its_figure_bar_after_bar() {
        // Every bar used to roll its own rhythm from its own stream, so no figure ever recurred
        // and a section had nothing in it to recognise.
        let (_, frame, parts) = draft(
            r#"
                form = "verse"
                chords = "@axis"
                humanize = 0
                variation = 0
                [section.verse]
                bars = 8
                [[part]]
                name = "lead"
                "#,
        );
        let lead = part(&parts, "lead");
        let figure = bar_steps(&frame, lead, 0);
        assert!(!figure.is_empty(), "the melody played nothing at all");

        let restated = (0..8)
            .filter(|bar| bar_steps(&frame, lead, *bar) == figure)
            .count();
        assert!(
            restated >= 4,
            "only {restated} of 8 bars restate the figure {figure:?}"
        );
    }

    #[test]
    fn the_melody_leaves_room_to_breathe() {
        // A note used to be held until the next onset or the bar line, so every bar was full of
        // sound from end to end and a phrase never finished — it only stopped.
        let (_, frame, parts) = draft(
            r#"
                form = "verse"
                chords = "@axis"
                humanize = 0
                [section.verse]
                bars = 8
                [[part]]
                name = "lead"
                "#,
        );
        let lead = part(&parts, "lead");
        let mut longest_rest = Ticks::ZERO;
        let mut sounded_to = Ticks::ZERO;
        for note in &lead.notes {
            longest_rest = longest_rest.max(note.start - sounded_to);
            sounded_to = sounded_to.max(note.start + note.length);
        }
        let beat = frame.grid.signature.ticks_per_beat();
        assert!(
            longest_rest >= beat,
            "the longest rest in eight bars is {} ticks, under one beat of {}",
            longest_rest.raw(),
            beat.raw()
        );
    }

    #[test]
    fn a_leap_is_filled_in_and_a_step_carries_on() {
        // The two regularities of `crate::melodic`. A memoryless walk has neither, and the
        // composer's line had neither: it drew every move from the same table whatever it had
        // just done.
        assert_eq!(shaped(3, 2, false), -2, "a leap up is answered downward");
        assert_eq!(shaped(-3, 1, false), 1, "and a leap down, upward");
        assert_eq!(
            shaped(2, 3, false),
            -2,
            "what fills a gap is a step, not a leap back"
        );
        assert_eq!(shaped(1, 1, false), 1, "a step up carries on up");
        assert_eq!(shaped(-1, 1, false), -1, "a step down carries on down");
        // A drawn leap after a step goes where it was drawn. Redirecting those too would leave a
        // figure that started upward with nothing able to turn it round.
        assert_eq!(shaped(1, -3, false), -3);
        // Neither rule is absolute, a repeated note is always allowed, and the first move of a
        // figure has nothing to answer.
        assert_eq!(shaped(3, 2, true), 2);
        assert_eq!(shaped(3, 0, false), 0);
        assert_eq!(shaped(0, 3, false), 3);
    }

    #[test]
    fn a_figure_that_has_wandered_is_turned_back() {
        // Regression to the mean, and the reason it is here: inertia gives the walk a direction,
        // and a walk with a direction meets the clamp and stands there repeating one note.
        assert_eq!(toward_middle(WANDER, 1), -1);
        assert_eq!(toward_middle(-WANDER, -2), 2);
        // Near the anchor it says nothing at all, and it never turns a move already coming home.
        assert_eq!(toward_middle(0, 3), 3);
        assert_eq!(toward_middle(WANDER, -1), -1);
    }

    #[test]
    fn a_restated_figure_joins_where_the_last_bar_left_off() {
        // Offsets paired with the pitch the figure's first note would land on. The join wants a
        // step, and the tie-break wants the smallest shift so the figure stays on its anchor.
        assert_eq!(join_offset(60, &[(-1, 55), (0, 59), (1, 64)]), 0);
        assert_eq!(join_offset(60, &[(-1, 62), (0, 67), (1, 71)]), -1);
        // A landing on the very note the last bar ended on loses to a step away from it: the
        // smoothest join available is always the one that does not move, and taking it outright
        // restated a third of all bars from a repeated note.
        assert_eq!(join_offset(60, &[(-1, 60), (0, 62), (1, 67)]), 0);
        // And to a third. The landings are chord-snapped now, so the nearest *other* landing is
        // usually a chord tone a third or fourth away — at the old rank of 3 the repeat beat
        // every one of those, and a fifth of all bar crossings still restated the last note.
        assert_eq!(join_offset(60, &[(-1, 60), (0, 63), (1, 67)]), 0);
        // Unless nothing else is anywhere near, where repeating beats leaping off the note.
        assert_eq!(join_offset(60, &[(-1, 60), (0, 67), (1, 69)]), -1);
    }

    #[test]
    fn a_repeat_the_range_made_is_unstuck_and_a_drawn_one_never_reaches_it() {
        // Near the edge of the range `shift_within` shrinks intervals, and two cells asking for
        // different degrees can arrive on one pitch — a repeat nobody drew, at the top of the
        // figure's arch. The move goes the way the figure was going, and turns back only at the
        // very edge, where that direction has no room left.
        let (_, frame, _) = draft(BASE);
        let scale = ChordScale::of_key(frame.sections[0].key);
        let (low, high) = Role::Melody.range();
        let moved = unstick(&scale, 76, 1, low, high);
        assert!(moved > 76, "going up with room above moves up: {moved}");
        assert!(scale.contains(PitchClass::new(moved)));
        // At the ceiling the intended direction has no room, so the note steps back instead —
        // still not a repeat, which is the whole point.
        let at_top = unstick(&scale, high, 1, low, high);
        assert!(
            at_top < high,
            "at the ceiling the walk turns back: {at_top}"
        );
        // A window of one note has nowhere to go at all, and says so.
        assert_eq!(unstick(&scale, 60, 1, 60, 60), 60);
    }

    #[test]
    fn the_tune_moves_mostly_by_step() {
        // The measurement the whole of `crate::melodic` is about, held at the size of one piece.
        // Before the rules above, a third of every melodic interval the composer wrote was a
        // fourth or wider — an arpeggio's distribution rather than a tune's.
        let mut intervals: Vec<i32> = Vec::new();
        for seed in 0..4u64 {
            let (_, _, parts) = draft(&format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    seed = {seed}
                    [section.verse]
                    bars = 8
                    [[part]]
                    name = "lead"
                    "#
            ));
            let lead = part(&parts, "lead");
            let mut notes = lead.notes.clone();
            notes.sort_by_key(|note| note.start.raw());
            intervals.extend(
                notes
                    .windows(2)
                    .map(|pair| i32::from(pair[1].pitch) - i32::from(pair[0].pitch)),
            );
        }
        assert!(intervals.len() >= 64, "too few notes to say anything");

        let wide = intervals.iter().filter(|i| i.abs() >= 5).count();
        assert!(
            wide * 4 <= intervals.len(),
            "{wide} of {} intervals are a fourth or wider",
            intervals.len()
        );
        let mean = intervals.iter().map(|i| i.abs()).sum::<i32>() as f32 / intervals.len() as f32;
        assert!(
            mean < 3.0,
            "the mean melodic interval is {mean:.2} semitones"
        );
    }

    #[test]
    fn a_figure_too_wide_for_the_range_shrinks_rather_than_folding() {
        // Folding moves a note a whole octave, which is a wider leap than any figure asks for and
        // usually in the opposite direction to the one it was going.
        let (_, frame, _) = draft(BASE);
        let section = &frame.sections[0];
        let (low, high) = Role::Melody.range();
        let anchor = high - 2;
        let pitch = shift_within(&ChordScale::of_key(section.key), anchor, 6, low, high);
        assert!(
            (low..=high).contains(&pitch),
            "{pitch} is outside the range"
        );
        assert!(
            pitch > anchor - OCTAVE,
            "a figure reaching upward was folded an octave down: {pitch} from {anchor}"
        );
    }

    /// Every melodic interval the composer writes, and the ones that cross a bar line.
    ///
    /// The measurement [`crate::melodic`] describes, run the way that page says to run it:
    /// [`compose`](crate::compose) over [`PRESETS`](crate::PRESETS) at four seeds each, the melody
    /// track picked out by [`Role::Melody`]'s colour, and the signed difference between
    /// consecutive pitches within each clip. Nothing here is a fixture — the interval
    /// distribution of a piece is a property of the piece.
    fn intervals() -> (Vec<i32>, Vec<i32>, Vec<i32>) {
        let colour = Role::Melody.color();
        let (mut all, mut crossing, mut inside) = (Vec::new(), Vec::new(), Vec::new());
        for preset in crate::preset::PRESETS {
            for seed in 0..4u64 {
                let mut spec = preset.spec();
                spec.seed = seed;
                let piece = crate::render::compose(&spec);
                let bar = piece.meter.ticks_per_bar().raw().max(1);
                for track in piece.tracks.iter().filter(|track| track.color == colour) {
                    for clip in &track.clips {
                        for pair in clip.notes.windows(2) {
                            let step = i32::from(pair[1].pitch) - i32::from(pair[0].pitch);
                            all.push(step);
                            match pair[0].start.raw() / bar == pair[1].start.raw() / bar {
                                true => inside.push(step),
                                false => crossing.push(step),
                            }
                        }
                    }
                }
            }
        }
        (all, crossing, inside)
    }

    #[test]
    fn the_tune_still_measures_like_a_tune() {
        // [`crate::melodic`] is a page of numbers with nothing checking them, and the constants in
        // this file are what it argues for: move `MOVE_WEIGHTS`, `shaped` or `join_offset` and the
        // page silently stops describing the composer. The bands are wide on purpose — this is
        // not a fingerprint, and taste is allowed to move inside them — but each one sits between
        // the figure that page reports and the figure it reports for the writer *before* those
        // rules existed, so a regression to the old shape cannot pass.
        let (all, crossing, inside) = intervals();
        let share = |count: usize| 100.0 * count as f64 / all.len() as f64;
        let mean = |steps: &[i32]| {
            let moved: Vec<i32> = steps.iter().copied().filter(|step| *step != 0).collect();
            moved.iter().map(|step| f64::from(step.abs())).sum::<f64>() / moved.len().max(1) as f64
        };

        // A third of every move being a fourth or wider is an arpeggio's distribution, not a
        // tune's. It was 34.3 per cent; the page reports 15.0 and a corpus 21 for all leaps.
        let leaps = share(all.iter().filter(|step| step.abs() >= 5).count());
        assert!(
            leaps < 18.0,
            "a leap of a fourth or wider is {leaps:.1}% of the line"
        );

        // Steps were 31.4 per cent, then 41.8 after the five rules; the page reports 53.3 after
        // the second pass took the accidental repeats out, against a corpus 68.
        let steps = share(
            all.iter()
                .filter(|step| (1..=2).contains(&step.abs()))
                .count(),
        );
        assert!(steps > 48.0, "only {steps:.1}% of the line moves by a step");
        assert!(steps > leaps, "the line leaps as readily as it steps");

        // A repeated note is a melodic move like any other and `MOVES` gives it one weight in
        // nine. The page reports 10.5 per cent against a corpus 11 — the second pass took out
        // the repeats the range clamp and the unsnapped join were writing, and the band guards
        // both directions: over 15 is the accidents coming back, and under 6 is something eating
        // the repeats the table legitimately draws.
        let repeats = share(all.iter().filter(|step| **step == 0).count());
        assert!(
            (6.0..15.0).contains(&repeats),
            "{repeats:.1}% of the line stands still"
        );

        // 3.89 before, 2.84 on the page, 2.8 in folk song with repetitions removed.
        assert!(mean(&all) < 3.0, "the mean interval is {:.2}", mean(&all));

        // The rule that was worth the most. The figure used to restart from its anchor every bar,
        // so the join between one bar and the next was whatever fell out of the difference
        // between two structural pitches and nothing had chosen it: a mean of 4.24 against 3.10
        // inside a bar. `join_offset` is what chooses it, and what the page claims is that the
        // crossings are now in line with the rest of the line rather than twice as wide.
        let (across, within) = (mean(&crossing), mean(&inside));
        assert!(
            across < within * 1.4,
            "a bar line costs {across:.2} semitones against {within:.2} inside one"
        );
    }
}
