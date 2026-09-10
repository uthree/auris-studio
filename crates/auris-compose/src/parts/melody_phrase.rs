//! A melodic response belongs to the call that precedes it, not to an isolated bar.

use super::{Motif, PhrasePlan};

/// Carries the written degree reached by the current phrase's preceding bar.
/// These are contour coordinates; the normal chord-scale and register passes realize them.
#[derive(Default)]
pub(super) struct PhraseLine {
    phrase_start: Option<usize>,
    endpoint: Option<i32>,
}

impl PhraseLine {
    /// State, echo, develop and answer a call using the already allotted rhythmic slots.
    ///
    /// A four-bar phrase has two statements, a continuation and an answer. Short phrases
    /// omit the echo; longer phrases continue towards the same answer without moving their
    /// breath. Only degrees change, after any rhythmic omission, ornament or closing hold.
    pub(super) fn develop(
        &mut self,
        call: &Motif,
        figure: &mut Motif,
        phrase: Option<&PhrasePlan>,
        bar: usize,
        inflection: i32,
    ) {
        let (Some(phrase), Some(first)) = (phrase, call.cells.first()) else {
            return;
        };
        if figure.cells.is_empty() {
            return;
        }
        if self.phrase_start != Some(phrase.start_bar) {
            self.phrase_start = Some(phrase.start_bar);
            self.endpoint = None;
        }
        let position = bar.saturating_sub(phrase.start_bar);
        let statements = if phrase.bars >= 4 { 2 } else { 1 };
        let flat = call.cells.iter().all(|cell| cell.degree == first.degree);
        if flat {
            // A deliberate reciting tone does not need an invented pitch excursion.
            for cell in &mut figure.cells {
                cell.degree = first.degree;
            }
        } else if position >= statements {
            let begin = self
                .endpoint
                .unwrap_or_else(|| call.cells.last().map_or(first.degree, |cell| cell.degree));
            let material = tail_steps(call);
            let goal = first.degree + inflection.clamp(-1, 1);
            if bar + 1 == phrase.end_bar() {
                answer(call, figure, begin, goal, &material);
            } else {
                let span = call
                    .cells
                    .iter()
                    .map(|cell| (cell.degree - first.degree).abs())
                    .max()
                    .unwrap_or(1)
                    .max(1);
                let direction = (first.degree - begin).signum();
                let direction = if direction == 0 {
                    // A call already back at its head turns away from its last approach.
                    material
                        .iter()
                        .rev()
                        .find(|step| **step != 0)
                        .map_or(0, |step| -step.signum())
                } else {
                    direction
                };
                let line = approach(
                    begin,
                    goal,
                    figure.cells.len() - 1,
                    &material,
                    span,
                    direction,
                );
                for (cell, degree) in figure.cells.iter_mut().zip(line) {
                    cell.degree = degree;
                }
            }
        }
        self.endpoint = figure.cells.last().map(|cell| cell.degree);
    }
}

/// The call's final gesture, including the step into its latter half.
/// A flat tail borrows the call's earlier moving gesture; a wholly flat call is handled above.
fn tail_steps(call: &Motif) -> Vec<i32> {
    let from = call.cells.len().div_ceil(2).saturating_sub(1);
    let mut steps: Vec<i32> = call.cells[from..]
        .windows(2)
        .map(|pair| pair[1].degree - pair[0].degree)
        .collect();
    if steps.iter().all(|step| *step == 0) {
        steps = call
            .cells
            .windows(2)
            .map(|pair| pair[1].degree - pair[0].degree)
            .collect();
    }
    steps
}

/// Fit a return and, when there is room, one excursion into the existing attacks.
///
/// A return fills wider source intervals by at most a third at a time, as the pitch walk's
/// gap-filling rule does. The unused travel can form one arch beyond the goal, bounded by the
/// call's own span. Reserving the remaining travel before each step prevents a final leap to
/// a destination the written notes cannot reach. Source repetitions retain their slots.
fn approach(
    begin: i32,
    goal: i32,
    moves: usize,
    material: &[i32],
    excursion: i32,
    direction: i32,
) -> Vec<i32> {
    let sizes: Vec<i32> = material
        .iter()
        .cycle()
        .take(moves)
        .map(|step| step.abs().min(2))
        .collect();
    let mut remaining: i32 = sizes.iter().sum();
    let goal = goal.clamp(begin - remaining, begin + remaining);
    let beyond = ((remaining - (goal - begin).abs()) / 2).min(excursion);
    let peak = goal + direction * beyond;
    let mut returning = false;
    let mut cursor = begin;
    let mut line = Vec::with_capacity(moves + 1);
    line.push(cursor);
    for size in sizes {
        remaining -= size;
        if cursor == peak {
            returning = true;
        }
        let target = if returning { goal } else { peak };
        let proposed = cursor + (target - cursor).clamp(-size, size);
        let next = proposed.clamp(goal - remaining, goal + remaining);
        if (next - cursor).signum() == -direction {
            returning = true;
        }
        cursor = next;
        line.push(cursor);
    }
    line
}

/// Recall the head's intervals from the continuation's endpoint, then return towards its origin.
fn answer(call: &Motif, figure: &mut Motif, begin: i32, goal: i32, material: &[i32]) {
    let head = call
        .cells
        .len()
        .div_ceil(2)
        .min(figure.cells.len().saturating_sub(1));
    let origin = call.cells[0].degree;
    for (cell, source) in figure.cells.iter_mut().take(head).zip(&call.cells) {
        cell.degree = begin + source.degree - origin;
    }
    let pivot = head
        .checked_sub(1)
        .map_or(begin, |index| figure.cells[index].degree);
    let moves = figure.cells.len() - head;
    // The answer can turn through a neighbouring scale note before arriving. Giving its
    // approach the same step/third allowance as gap filling avoids reaching the goal early
    // and merely repeating it through every remaining attack. Deliberate source repeats stay.
    let approach_steps: Vec<i32> = material.iter().map(|step| 2 * step.signum()).collect();
    let direction = (goal - pivot).signum();
    let direction = if direction == 0 {
        material
            .iter()
            .rev()
            .find(|step| **step != 0)
            .map_or(0, |step| -step.signum())
    } else {
        direction
    };
    let line = approach(pivot, goal, moves, &approach_steps, 1, direction);
    for (cell, degree) in figure.cells[head..]
        .iter_mut()
        .zip(line.into_iter().skip(1))
    {
        cell.degree = degree;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phrasing::PhraseRole;
    use crate::rhythm::Accent;

    fn motif(degrees: &[i32]) -> Motif {
        Motif {
            cells: degrees
                .iter()
                .enumerate()
                .map(|(index, degree)| super::super::Cell {
                    step: index * 2,
                    length: 2,
                    accent: if index == 0 {
                        Accent::Strong
                    } else {
                        Accent::Normal
                    },
                    degree: *degree,
                })
                .collect(),
        }
    }

    fn pitches(figure: &Motif) -> Vec<i32> {
        figure.cells.iter().map(|cell| cell.degree).collect()
    }

    fn phrase(bars: usize) -> PhrasePlan {
        PhrasePlan {
            start_bar: 0,
            bars,
            role: PhraseRole::Statement,
        }
    }

    fn written_phrase(call: &Motif, bars: usize) -> Vec<Motif> {
        let plan = phrase(bars);
        let mut line = PhraseLine::default();
        (0..bars)
            .map(|bar| {
                let mut figure = call.clone();
                line.develop(call, &mut figure, Some(&plan), bar, 0);
                figure
            })
            .collect()
    }

    #[test]
    fn a_monotone_call_returns_through_its_own_steps_instead_of_restarting() {
        let call = motif(&[0, -1, -2, -3]);
        let bars = written_phrase(&call, 4);
        assert_eq!(pitches(&bars[0]), pitches(&call));
        assert_eq!(pitches(&bars[1]), pitches(&call));
        assert_eq!(pitches(&bars[2]), [-3, -2, -1, 0]);
        assert_eq!(
            bars[3].cells[0].degree,
            bars[2].cells.last().unwrap().degree
        );
        assert_eq!(bars[3].cells[1].degree - bars[3].cells[0].degree, -1);
        assert_eq!(bars[3].cells.last().unwrap().degree, 0);
        assert!(
            bars[2..]
                .iter()
                .flat_map(|bar| bar.cells.windows(2))
                .all(|pair| (pair[1].degree - pair[0].degree).abs() <= 2)
        );
    }

    #[test]
    fn changing_the_call_tail_changes_its_continuation() {
        let first = written_phrase(&motif(&[0, -1, -2, -3]), 4);
        let second = written_phrase(&motif(&[0, -1, -1, 1]), 4);
        assert_ne!(pitches(&first[2]), pitches(&second[2]));
    }

    #[test]
    fn an_answer_is_connected_to_the_endpoint_it_was_given() {
        let call = motif(&[0, -1, -2, -3]);
        let mut low = call.clone();
        let mut high = call.clone();
        answer(&call, &mut low, -2, 0, &tail_steps(&call));
        answer(&call, &mut high, 2, 0, &tail_steps(&call));
        let relative = |figure: &Motif| {
            pitches(figure)
                .iter()
                .map(|degree| degree - figure.cells[0].degree)
                .collect::<Vec<_>>()
        };
        assert_ne!(relative(&low), relative(&high));
    }

    #[test]
    fn phrase_development_is_independent_of_the_degree_origin() {
        for degrees in [&[0, -1, -2, -3][..], &[0, 2, 1, -1, -2, -1], &[0, 0, 0, 0]] {
            let call = motif(degrees);
            let shifted = motif(&degrees.iter().map(|degree| degree + 7).collect::<Vec<_>>());
            for bars in [1, 2, 3, 4, 5, 8] {
                for (original, transposed) in written_phrase(&call, bars)
                    .iter()
                    .zip(written_phrase(&shifted, bars))
                {
                    assert_eq!(
                        pitches(&transposed),
                        pitches(original)
                            .iter()
                            .map(|degree| degree + 7)
                            .collect::<Vec<_>>()
                    );
                }
            }
        }
    }

    #[test]
    fn a_reciting_tone_remains_flat_through_every_phrase_role() {
        let call = motif(&[3, 3, 3, 3]);
        for role in [
            PhraseRole::Statement,
            PhraseRole::Continuation,
            PhraseRole::Answer,
            PhraseRole::Release,
        ] {
            let mut line = PhraseLine::default();
            let plan = PhrasePlan { role, ..phrase(8) };
            for bar in 0..8 {
                let mut figure = call.clone();
                line.develop(&call, &mut figure, Some(&plan), bar, 1);
                assert_eq!(pitches(&figure), [3, 3, 3, 3]);
            }
        }
    }

    #[test]
    fn approaches_reserve_their_final_step_without_inventing_a_last_note_leap() {
        for begin in -6..=6 {
            for goal in -6..=6 {
                for material in [&[-1][..], &[-2, -1], &[1, 0, 2], &[3, -1, -2]] {
                    for moves in 0..16 {
                        let line = approach(begin, goal, moves, material, 3, 1);
                        let budget: i32 = material
                            .iter()
                            .cycle()
                            .take(moves)
                            .map(|step| step.abs().min(2))
                            .sum();
                        assert_eq!(
                            *line.last().unwrap(),
                            goal.clamp(begin - budget, begin + budget)
                        );
                        assert_eq!(line.len(), moves + 1);
                        for (pair, step) in line.windows(2).zip(material.iter().cycle()) {
                            assert!((pair[1] - pair[0]).abs() <= step.abs().min(2));
                        }
                    }
                }
            }
        }
    }
}
