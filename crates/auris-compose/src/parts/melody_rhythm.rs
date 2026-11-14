//! Rhythmic gestures whose arrivals are placed in the context of the whole bar.

use super::{Accent, Cell, Grid, Motif, PerformanceStyle, Rng};

/// Fill a short run with evenly spaced subdivisions, retaining a longer target beside it.
fn subdivisions(start: usize, length: usize, count: usize, onsets: &mut Vec<usize>) {
    for index in 0..count.min(length).max(1) {
        onsets.push(start + index * length / count.min(length).max(1));
    }
}

/// Which bar of the two-bar figure a rhythm is drawn for.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Half {
    /// The bar that states the figure.
    Call,
    /// The bar that answers it: one subdivision calmer, and more often than not coming to
    /// rest on a note held through its last two beats.
    Answer,
}

/// How often the answering bar ends on a held note rather than on a gesture of its own.
///
/// Sung lines alternate: measured over the PDMX pop and rock vocal parts, a bar differs from
/// the bar before it by two notes on average, and one note in eleven lasts two beats or more.
/// Two bars drawn the same way differ by one and hold nothing, so the answer is drawn calmer
/// — and this often it simply stops and holds, which is where the tune's long notes come from.
const ANSWER_HOLDS: f32 = 0.6;

/// How often an answer that comes to rest does it on the note it was already on, striking
/// nothing new in its last two beats.
///
/// Measured over the same corpus, 18 half-bars in a hundred have no note starting in them at
/// all: a held note going on, or a rest. The composer's had five, because every gesture opened
/// with a note of its own, and the longest thing it could write was two beats.
const ANSWER_CARRIES: f32 = 0.5;

/// How many times an answer that came out as the call's own rhythm is drawn again.
///
/// The two bars share one table, so now and then they draw the same thing and the figure is
/// one bar long after all. A few more draws settle nearly every case; a table with one
/// outcome cannot be argued with, and after these the bars are allowed to agree.
const ANSWER_REDRAWS: usize = 4;

/// Draw gestures, then allocate their approaches against the following gesture's arrival.
pub(super) fn grouped(
    grid: Grid,
    density: f32,
    syncopation: f32,
    style: Option<PerformanceStyle>,
    rng: &mut Rng,
) -> Vec<Cell> {
    gestures(grid, density, syncopation, style, Half::Call, rng)
}

/// The rhythm of the bar that answers `call`, drawn so that the two are not the same bar.
///
/// What makes a figure two bars long rather than one bar said twice. The answer is drawn from
/// the same gestures as the call, a subdivision calmer and inclined to end on a held note —
/// see [`ANSWER_HOLDS`] — and drawn again when it lands on the call's own onsets.
pub(super) fn answering(
    grid: Grid,
    density: f32,
    syncopation: f32,
    style: Option<PerformanceStyle>,
    call: &[Cell],
    rng: &mut Rng,
) -> Vec<Cell> {
    let onsets = |cells: &[Cell]| cells.iter().map(|cell| cell.step).collect::<Vec<_>>();
    let mut answer = gestures(grid, density, syncopation, style, Half::Answer, rng);
    for _ in 0..ANSWER_REDRAWS {
        if onsets(&answer) != onsets(call) {
            break;
        }
        answer = gestures(grid, density, syncopation, style, Half::Answer, rng);
    }
    answer
}

/// How often a gesture is drawn on the grid's own step rather than on the meter's offbeat.
///
/// Nothing below a density of 0.15, and rising from there: at the density a clip starts with,
/// about one of a call's gestures in five. Measured over the PDMX pop and rock vocal parts, 12
/// notes in a hundred follow the one before by a sixteenth and 22 bars in a hundred have a
/// note off the eighth at all. The composer had 22 and 47, because the shortest note was
/// whatever a run came to when it was divided: see the seventh pass in [`crate::melodic`].
/// Chiptune reaches for the fine grid sooner, and the sustained palettes later.
fn fineness(density: f32, style: Option<PerformanceStyle>) -> f32 {
    let base = ((density - 0.15) * 0.8).clamp(0.0, 0.6);
    match style {
        Some(PerformanceStyle::Chiptune) => base + 0.25,
        Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral) => 0.5 * base,
        _ => base,
    }
}

/// One bar of gestures for either half of the figure.
fn gestures(
    grid: Grid,
    density: f32,
    syncopation: f32,
    style: Option<PerformanceStyle>,
    half: Half,
    rng: &mut Rng,
) -> Vec<Cell> {
    let steps = grid.steps_per_bar();
    let beat = grid.steps_per_beat().min(steps).max(1);
    let density = density.clamp(0.0, 1.0);
    let syncopation = syncopation.clamp(0.0, 1.0);
    // Chiptune approaches reach their subdivisions sooner while retaining the
    // held answer. Other palettes keep the slower progression to dense runs.
    let subdivision_range = match style {
        Some(PerformanceStyle::Chiptune) => 4.0,
        _ => 3.0,
    };
    let splits = (1 + (density * subdivision_range).round() as usize).min(beat.min(4));
    let (splits, fineness) = match half {
        Half::Call => (splits, fineness(density, style)),
        Half::Answer => (
            splits.saturating_sub(1).max(1),
            0.5 * fineness(density, style),
        ),
    };
    // Held, short-long, long-short, anticipation, pickup, running. Styles choose gestures,
    // while the meter supplies their size; a dotted quarter in 6/8 remains a whole felt beat.
    let weights = match style {
        Some(PerformanceStyle::Ambient) => [8.0, 1.0, 1.0, 0.5, 0.5, 0.2],
        Some(PerformanceStyle::Orchestral) => [4.0, 2.0, 2.0, 0.5, 0.5, 0.5],
        Some(PerformanceStyle::JazzTrio | PerformanceStyle::CityPop) => [
            0.5,
            1.5,
            2.0,
            2.0 + 3.0 * syncopation,
            2.0,
            1.0 + 2.0 * density,
        ],
        Some(PerformanceStyle::Rock | PerformanceStyle::Chiptune) => {
            [0.5, 3.0, 3.0, 0.5 + syncopation, 1.0, 1.5 + 3.0 * density]
        }
        _ => [
            0.5,
            2.0,
            2.5,
            1.0 + 3.0 * syncopation,
            1.0,
            1.0 + 3.0 * density,
        ],
    };
    let offbeat = offbeat_steps(grid).unwrap_or(1).max(1);
    let mut cells: Vec<Cell> = Vec::new();
    for start in (0..steps).step_by(2 * beat) {
        let end = (start + 2 * beat).min(steps);
        let length = end - start;
        let mut onsets = Vec::new();
        let mut target = start;
        // The shortest note this gesture may write: the meter's offbeat, or a step of the grid
        // when the gesture was drawn fine. Rolled for every gesture so the stream stays aligned.
        let fine = rng.chance(fineness);
        let unit = if fine { 1 } else { offbeat };
        if length <= beat {
            // An unmatched beat in an odd meter finishes the cell with one held arrival.
            onsets.push(start);
        } else {
            let middle = start + beat;
            let short = (beat / 2).max(1);
            // No more notes in a run than its unit leaves room for, and one more than the
            // density asked for when the gesture is fine — which is what a fine gesture is.
            let run = |from: usize, length: usize, onsets: &mut Vec<usize>| {
                let count = splits + usize::from(fine);
                subdivisions(from, length, count.min((length / unit).max(1)), onsets);
            };
            // Straight on by the offbeat from `from` to the end of the gesture.
            let through = |from: usize, onsets: &mut Vec<usize>| {
                subdivisions(from, end - from, ((end - from) / offbeat).max(1), onsets);
            };
            // The answer's last gesture is where it comes to rest. Both rolls are made whether
            // or not the hold is taken, so the stream stays aligned either way.
            let drawn = rng.weighted(&weights);
            let holds = rng.chance(ANSWER_HOLDS);
            let carries = rng.chance(ANSWER_CARRIES);
            let last = end == steps;
            let resting = half == Half::Answer && last && holds;
            if resting
                && carries
                && let Some(held) = cells.last_mut()
            {
                // The note the bar was already on is the one it rests on: nothing new is
                // struck, and it sounds to the end of the bar.
                held.length = end - held.step;
                continue;
            }
            // A bar's last gesture never runs: it is where the bar's held note is, and a bar
            // of nothing but eighths has nowhere it was going.
            let kind = match drawn {
                _ if resting => 0,
                5 if last => 1,
                kind => kind,
            };
            match kind {
                0 => onsets.push(start),
                1 => {
                    run(start, beat, &mut onsets);
                    onsets.push(middle);
                    target = middle;
                }
                2 => {
                    onsets.push(start);
                    run(middle, end - middle, &mut onsets);
                }
                3 if beat > 1 && last => {
                    // Early, and held to the end of the bar.
                    target = middle - short;
                    run(start, target - start, &mut onsets);
                    onsets.push(target);
                }
                3 if beat > 1 => {
                    // Late: an offbeat ahead of the gesture that follows, off the back of an
                    // approach in the first beat.
                    target = end - offbeat.min(beat - 1);
                    run(start, beat, &mut onsets);
                    onsets.push(target);
                }
                5 => {
                    // Arriving on whatever the next gesture opens with.
                    through(start, &mut onsets);
                    target = onsets.last().copied().unwrap_or(start);
                }
                _ => {
                    let pickup = start + short.min(beat - 1);
                    if last {
                        run(pickup, middle - pickup, &mut onsets);
                        onsets.push(middle);
                        target = middle;
                    } else {
                        // A pickup inside the bar carries on into the gesture that follows.
                        through(pickup, &mut onsets);
                        target = onsets.last().copied().unwrap_or(pickup);
                    }
                }
            }
        }
        // Move a short preparation off the beat, keeping the held target intact. Changing
        // only palette weights can select the same rhythm at both ends of the syncopation dial.
        // The delay is the gesture's own unit: pushing a note a step late inside a run of
        // offbeats wrote a sixteenth nobody had asked for.
        if beat > 1 && rng.chance(syncopation) {
            for position in 0..onsets.len() {
                let step = onsets[position];
                let next = onsets.get(position + 1).copied().unwrap_or(end);
                let held = step == target;
                let room = next - step;
                if step.is_multiple_of(beat) && room > if held { beat } else { unit } {
                    let delay = (beat / 2).max(1).min(room - if held { beat } else { unit });
                    onsets[position] += delay;
                    if held {
                        target += delay;
                    }
                    break;
                }
            }
        }
        for (position, step) in onsets.iter().enumerate() {
            let next = onsets.get(position + 1).copied().unwrap_or(end);
            cells.push(Cell {
                step: *step,
                accent: if *step == target {
                    Accent::Strong
                } else {
                    Accent::Normal
                },
                length: next - step,
                degree: 0,
            });
        }
    }
    connect_gestures(grid, style, &mut cells);
    cells
}

/// Spread a busy preparation towards its answer instead of stopping early in the bar.
///
/// An anticipated target is provisional until the following gesture is known. Two short
/// attacks followed by a long hold and a pickup rest can otherwise leave two felt beats
/// without a new attack. Retiming the existing approach preserves its contour and density;
/// a sparse hold, a late arrival and a sustained palette have no such obligation to move.
fn connect_gestures(grid: Grid, style: Option<PerformanceStyle>, cells: &mut [Cell]) {
    if matches!(
        style,
        Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral)
    ) {
        return;
    }
    let steps = grid.steps_per_bar();
    let beat = grid.steps_per_beat().max(1);
    for boundary in (2 * beat..steps).step_by(2 * beat) {
        let start = cells.partition_point(|cell| cell.step < boundary - 2 * beat);
        let next = cells.partition_point(|cell| cell.step < boundary);
        if next - start < 3 || next == cells.len() {
            continue;
        }
        let target = next - 1;
        let following = cells[next].step;
        if cells[target].length < beat || following - cells[target].step <= beat {
            continue;
        }
        let arrival = following.saturating_sub(beat).min(boundary - 1);
        let first = cells[start].step;
        let end = cells[target].step + cells[target].length;
        if arrival <= cells[target].step || arrival >= end {
            continue;
        }
        // Prefer felt beats, then their natural subdivisions. Evenly stretching the old
        // run would invent dotted subdivisions on a straight grid. The pickup stays put.
        let mut preparation: Vec<usize> = (first + 1..arrival).collect();
        preparation.sort_by_key(|step| {
            // Grid::weight distinguishes beats and their first subdivisions. Below that,
            // the common divisor prefers nested subdivisions on fine or triplet grids.
            let mut divisor = beat;
            let mut phase = step % beat;
            while phase != 0 {
                (divisor, phase) = (phase, divisor % phase);
            }
            (
                std::cmp::Reverse(grid.weight(*step)),
                std::cmp::Reverse(divisor),
                *step,
            )
        });
        preparation.truncate(target - start - 1);
        preparation.push(first);
        preparation.push(arrival);
        preparation.sort_unstable();
        for (cell, step) in cells[start..next].iter_mut().zip(preparation) {
            cell.step = step;
        }
        // Only the approach is reflowed. The following gesture keeps its deliberate pickup
        // rest, and a liked late held target keeps both its timing and its articulation.
        for index in start..target {
            cells[index].length = cells[index + 1].step - cells[index].step;
        }
        cells[target].length = end - arrival;
    }
}

/// The meter's own offbeat in steps: half a beat, or a third of one where the beat divides in
/// three. `None` on a grid too coarse to have one.
pub(super) fn offbeat_steps(grid: Grid) -> Option<usize> {
    let beat = grid.steps_per_beat();
    let division = if grid.signature.is_compound() || grid.is_triplet() {
        3
    } else {
        2
    };
    (beat.is_multiple_of(division) && beat >= division).then_some(beat / division)
}

/// How a phrase is let go of.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum Ending {
    /// One beat on the arrival: the line pauses and means to go on.
    Open,
    /// Two beats on it: the line has got where it was going.
    Closed,
    /// A beat and an offbeat on it, let go a whole beat before the pickup that leads into the
    /// next phrase — so the pickup is heard as the next phrase starting and not as this one
    /// carrying on.
    Early,
}

/// Place a held arrival before the final felt beat, which is reserved for the next phrase.
///
/// Every phrase used to end the same way — a beat of note and a beat of rest — so a section
/// was four identical cadences in a row, and the longest note in sixteen bars averaged under a
/// beat and a half where a sung line's averages over three. `ending` says which of three ways
/// this one goes. The two longer ones arrive a beat sooner; a bar with fewer than four felt
/// beats has no beat to arrive sooner on, and closes as an open one does.
pub(super) fn close_phrase(grid: Grid, figure: &mut Motif, ending: Ending) {
    let steps = grid.steps_per_bar();
    let beat = grid.steps_per_beat();
    if steps <= beat || figure.cells.is_empty() {
        return;
    }
    let arrival_degree = figure.cells.last().map_or(0, |cell| cell.degree);
    let roomy = steps >= 4 * beat;
    let (release, held) = match (ending, offbeat_steps(grid)) {
        (Ending::Closed, _) if roomy => (steps - beat, 2 * beat),
        (Ending::Early, Some(offbeat)) if roomy => (steps - beat - offbeat, beat + offbeat),
        _ => (steps - beat, beat),
    };
    let target = release.saturating_sub(held);
    let earliest = target.saturating_sub(beat / 2);
    if let Some(index) = figure.cells.iter().rposition(|cell| cell.step <= target) {
        figure.cells.truncate(index + 1);
        if index >= 2 && figure.cells[index].step < earliest {
            // An early preparation is not the final arrival. Keep the approach and give
            // the final degree its own landing, rather than stretching that preparation
            // across most of the bar. A half-beat anticipated arrival is already valid.
            let previous = &mut figure.cells[index];
            previous.length = previous.length.min(target - previous.step);
            figure.cells.push(Cell {
                step: target,
                length: release - target,
                degree: arrival_degree,
                accent: Accent::Strong,
            });
        }
    } else {
        // A pickup may begin after the available arrival in a two-beat bar. Move that one
        // arrival onto the downbeat; retaining the pickup would consume its own breath.
        figure.cells.truncate(1);
        figure.cells[0].step = 0;
    }
    if let Some(last) = figure.cells.last_mut() {
        last.length = release - last.step;
        last.accent = Accent::Strong;
        last.degree = arrival_degree;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_core::time::TimeSignature;

    fn cells_at(positions: &[(usize, usize)]) -> Vec<Cell> {
        positions
            .iter()
            .enumerate()
            .map(|(degree, &(step, length))| Cell {
                step,
                length,
                degree: degree as i32,
                accent: Accent::Normal,
            })
            .collect()
    }

    fn signature(cells: &[Cell]) -> Vec<(usize, usize, i32, Accent)> {
        cells
            .iter()
            .map(|cell| (cell.step, cell.length, cell.degree, cell.accent))
            .collect()
    }

    #[test]
    fn an_early_busy_arrival_is_retimed_without_adding_notes_or_filling_the_pickup() {
        let mut cells = cells_at(&[(0, 1), (1, 1), (2, 6), (10, 1), (11, 1), (12, 4)]);
        cells[2].accent = Accent::Strong;
        connect_gestures(Grid::default(), Some(PerformanceStyle::PopBand), &mut cells);
        assert_eq!(
            signature(&cells),
            vec![
                (0, 4, 0, Accent::Normal),
                (4, 2, 1, Accent::Normal),
                (6, 2, 2, Accent::Strong),
                (10, 1, 3, Accent::Normal),
                (11, 1, 4, Accent::Normal),
                (12, 4, 5, Accent::Normal),
            ]
        );
    }

    #[test]
    fn late_arrivals_flowing_heads_and_sparse_holds_keep_their_rhythm() {
        for positions in [
            &[(0, 4), (4, 2), (6, 2), (10, 6)][..],
            &[(0, 2), (2, 2), (4, 4), (8, 2), (10, 2), (12, 4)],
            &[(0, 8), (8, 4), (12, 4)],
        ] {
            let mut cells = cells_at(positions);
            let before = signature(&cells);
            connect_gestures(Grid::default(), Some(PerformanceStyle::PopBand), &mut cells);
            assert_eq!(signature(&cells), before);
        }
    }

    #[test]
    fn sustained_palettes_can_keep_an_early_held_target() {
        for style in [PerformanceStyle::Ambient, PerformanceStyle::Orchestral] {
            let mut cells = cells_at(&[(0, 1), (1, 1), (2, 6), (10, 6)]);
            let before = signature(&cells);
            connect_gestures(Grid::default(), Some(style), &mut cells);
            assert_eq!(signature(&cells), before);
        }
    }

    #[test]
    fn redistributing_an_approach_keeps_its_leading_pickup() {
        let mut cells = cells_at(&[(1, 1), (2, 1), (3, 5), (10, 6)]);
        connect_gestures(Grid::default(), Some(PerformanceStyle::CityPop), &mut cells);
        assert_eq!(
            cells.iter().map(|cell| cell.step).collect::<Vec<_>>(),
            [1, 4, 6, 10]
        );
    }

    #[test]
    fn fine_grids_prefer_nested_subdivisions_over_an_initial_cluster() {
        let grid = Grid::new(TimeSignature::default(), 8);
        let mut cells = cells_at(&[(0, 1), (1, 1), (2, 1), (3, 1), (4, 12), (20, 12)]);
        connect_gestures(grid, Some(PerformanceStyle::PopBand), &mut cells);
        assert_eq!(
            cells.iter().map(|cell| cell.step).collect::<Vec<_>>(),
            [0, 2, 4, 8, 12, 20]
        );
    }

    #[test]
    fn the_answer_is_not_the_call_and_is_the_calmer_of_the_two() {
        // Two bars drawn the same way are one bar's rhythm twice more often than not, and
        // differ by a note when they differ. The answer is what makes the figure two bars
        // long: its own onsets, fewer of them, and usually a note to come to rest on.
        let grid = Grid::default();
        let two_beats = 2 * grid.steps_per_beat();
        let (mut called, mut answered, mut same, mut resting) = (0, 0, 0, 0);
        for seed in 0..64 {
            let mut rng = Rng::stream(seed, &[]);
            let call = grouped(grid, 0.5, 0.3, Some(PerformanceStyle::PopBand), &mut rng);
            let answer = answering(
                grid,
                0.5,
                0.3,
                Some(PerformanceStyle::PopBand),
                &call,
                &mut rng,
            );
            assert!(!answer.is_empty(), "seed {seed} answers with nothing");
            assert!(
                answer
                    .iter()
                    .all(|cell| cell.length > 0 && cell.step + cell.length <= grid.steps_per_bar()),
                "seed {seed} answers past its own bar: {answer:?}"
            );
            assert!(
                answer
                    .windows(2)
                    .all(|pair| pair[0].step + pair[0].length <= pair[1].step),
                "seed {seed} answers with overlapping notes: {answer:?}"
            );
            called += call.len();
            answered += answer.len();
            let onsets = |cells: &[Cell]| cells.iter().map(|cell| cell.step).collect::<Vec<_>>();
            same += usize::from(onsets(&call) == onsets(&answer));
            resting += usize::from(answer.iter().any(|cell| cell.length >= two_beats));
        }
        assert!(same <= 1, "{same} of 64 answers are the call's own rhythm");
        assert!(
            answered * 10 < called * 9,
            "the answers hold {answered} notes against the calls' {called}"
        );
        assert!(
            (24..=56).contains(&resting),
            "{resting} of 64 answers come to rest on a note of two beats"
        );
    }

    /// The call and its answer for `seed`, as the figure draws them.
    fn figure(seed: u64, density: f32) -> (Vec<Cell>, Vec<Cell>) {
        let grid = Grid::default();
        let style = Some(PerformanceStyle::PopBand);
        let mut rng = Rng::stream(seed, &[]);
        let call = grouped(grid, density, 0.3, style, &mut rng);
        let answer = answering(grid, density, 0.3, style, &call, &mut rng);
        (call, answer)
    }

    #[test]
    fn a_gesture_is_written_in_offbeats_until_the_density_asks_for_finer() {
        // The shortest note used to be a matter of arithmetic: an approach half a beat long
        // split in two is a pair of sixteenths, so the commonest figure in a section at any
        // ordinary density was three sixteenths running, which a sung line all but never has.
        // A gesture is on the offbeat unless it was drawn fine, and how often that is belongs
        // to the density.
        let odd = |density: f32| {
            (0..64)
                .flat_map(|seed| {
                    let (call, answer) = figure(seed, density);
                    call.into_iter().chain(answer)
                })
                .filter(|cell| cell.step % 2 == 1)
                .count()
        };
        assert_eq!(odd(0.1), 0, "a calm figure has a note off the eighth");
        assert!(odd(0.9) > odd(0.45), "density does not make a figure finer");
        assert!(odd(0.45) > 0, "an ordinary figure is never fine");
    }

    #[test]
    fn note_values_are_a_singers_rather_than_an_arpeggiators() {
        // The seventh pass of `crate::melodic`. Counted over the PDMX pop and rock vocal parts,
        // 12 of every hundred notes follow the one before by a sixteenth and 48 by an eighth;
        // the composer had 22 and 34. Each band sits between what the writer does now and what
        // it did, and guards the other side as well.
        let (mut sixteenths, mut eighths, mut spacings) = (0, 0, 0);
        let (mut running, mut carried, mut bars) = (0, 0, 0);
        let grid = Grid::default();
        let steps = grid.steps_per_bar();
        for seed in 0..256 {
            let (call, answer) = figure(seed, 0.45);
            for bar in [&call, &answer] {
                bars += 1;
                for pair in bar.windows(2) {
                    spacings += 1;
                    sixteenths += usize::from(pair[1].step - pair[0].step == 1);
                    eighths += usize::from(pair[1].step - pair[0].step == 2);
                }
                let onsets: Vec<usize> = bar.iter().map(|cell| cell.step).collect();
                // Four eighths straight through the first half of a bar: a tenth of a sung
                // line's half-bars, and something the gestures could not write at all.
                running += usize::from(onsets.starts_with(&[0, 2, 4, 6]));
                assert!(
                    !onsets.ends_with(&[8, 10, 12, 14]),
                    "seed {seed} runs through the half of the bar that holds: {onsets:?}"
                );
                // Nothing struck in the second half, and the note before it held through.
                if onsets.iter().all(|step| *step < steps / 2) {
                    carried += 1;
                    let last = bar.last().unwrap();
                    assert_eq!(last.step + last.length, steps, "seed {seed}: {bar:?}");
                }
            }
        }
        let percent = |count: usize, of: usize| 100.0 * count as f64 / of as f64;
        let (sixteenths, eighths) = (percent(sixteenths, spacings), percent(eighths, spacings));
        assert!(
            (3.0..16.0).contains(&sixteenths),
            "{sixteenths:.1}% of notes follow by a sixteenth"
        );
        assert!(
            (42.0..66.0).contains(&eighths),
            "{eighths:.1}% of notes follow by an eighth"
        );
        let (running, carried) = (percent(running, bars), percent(carried, bars));
        assert!(
            (5.0..35.0).contains(&running),
            "{running:.1}% of bars open with four eighths"
        );
        assert!(
            (5.0..30.0).contains(&carried),
            "{carried:.1}% of bars strike nothing in their second half"
        );
    }

    #[test]
    fn a_phrase_is_let_go_of_in_one_of_three_ways() {
        let grid = Grid::default();
        let ended = |ending: Ending, grid: Grid, positions: &[(usize, usize)]| {
            let mut figure = Motif {
                cells: cells_at(positions),
            };
            close_phrase(grid, &mut figure, ending);
            let last = *figure.cells.last().unwrap();
            (figure.cells.len(), last.step, last.length, last.degree)
        };
        let busy = [(0, 2), (2, 2), (4, 2), (6, 2), (8, 4), (12, 4)];
        // A beat on beat three, and the last beat left empty.
        assert_eq!(ended(Ending::Open, grid, &busy), (5, 8, 4, 5));
        // Two beats from beat two: the same breath after a longer arrival.
        assert_eq!(ended(Ending::Closed, grid, &busy), (3, 4, 8, 5));
        // A beat and a half from beat two, let go a whole beat before the pickup's offbeat.
        assert_eq!(ended(Ending::Early, grid, &busy), (3, 4, 6, 5));
        // Three beats have no beat to arrive sooner on, so every ending is the open one.
        let waltz = Grid::new(TimeSignature::new(3, 4), 4);
        let short = [(0, 2), (2, 2), (4, 4), (8, 4)];
        let open = ended(Ending::Open, waltz, &short);
        assert_eq!(ended(Ending::Closed, waltz, &short), open);
        assert_eq!(ended(Ending::Early, waltz, &short), open);
    }

    #[test]
    fn sparse_phrase_endings_do_not_gain_an_extra_attack() {
        let mut figure = Motif {
            cells: cells_at(&[(0, 16)]),
        };
        close_phrase(Grid::default(), &mut figure, Ending::Open);
        assert_eq!(signature(&figure.cells), [(0, 12, 0, Accent::Strong)]);
    }

    #[test]
    fn a_busy_opening_does_not_spend_two_beats_waiting_for_the_answer() {
        let grid = Grid::default();
        let middle = grid.steps_per_bar() / 2;
        for seed in 0..128 {
            let cells = grouped(
                grid,
                0.55,
                0.65,
                Some(PerformanceStyle::PopBand),
                &mut Rng::stream(seed, &[]),
            );
            if cells.iter().filter(|cell| cell.step < middle).count() < 3 {
                continue;
            }
            for pair in cells.windows(2) {
                if pair[0].step < middle && pair[1].step >= middle {
                    assert!(
                        pair[1].step - pair[0].step < 2 * grid.steps_per_beat(),
                        "seed {seed} stops in mid-phrase: {cells:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_closing_answer_does_not_stretch_the_early_preparation_into_the_cadence() {
        let grid = Grid::default();
        let mut figure = Motif {
            cells: cells_at(&[(0, 1), (1, 1), (2, 6), (10, 1), (11, 1), (12, 4)]),
        };
        close_phrase(grid, &mut figure, Ending::Open);
        let last = figure.cells.last().unwrap();
        assert!(
            last.step >= grid.steps_per_bar() / 2 - grid.steps_per_beat() / 2,
            "the answer fell back to the opening: {figure:?}"
        );
        assert_eq!(last.degree, 5);
    }

    #[test]
    fn cells_and_arrivals_fit_odd_compound_and_triplet_grids() {
        let signatures = TimeSignature::COMMON
            .into_iter()
            .chain([TimeSignature::new(1, 16)]);
        for signature in signatures {
            for subdivision in [1, 2, 3, 4, 6, 8] {
                let grid = Grid::new(signature, subdivision);
                for style in [
                    None,
                    Some(PerformanceStyle::PopBand),
                    Some(PerformanceStyle::Rock),
                    Some(PerformanceStyle::Chiptune),
                    Some(PerformanceStyle::CityPop),
                    Some(PerformanceStyle::JazzTrio),
                    Some(PerformanceStyle::Synthwave),
                    Some(PerformanceStyle::Orchestral),
                    Some(PerformanceStyle::Ambient),
                ] {
                    for seed in 0..16 {
                        let mut figure = Motif {
                            cells: grouped(
                                grid,
                                seed as f32 / 15.0,
                                0.7,
                                style,
                                &mut Rng::stream(seed, &[]),
                            ),
                        };
                        assert!(!figure.cells.is_empty());
                        assert!(figure.cells.iter().all(|cell| {
                            cell.length > 0 && cell.step + cell.length <= grid.steps_per_bar()
                        }));
                        assert!(
                            figure
                                .cells
                                .windows(2)
                                .all(|pair| { pair[0].step + pair[0].length <= pair[1].step })
                        );
                        close_phrase(grid, &mut figure, Ending::Open);
                        let last = figure.cells.last().unwrap();
                        let limit = if grid.steps_per_bar() > grid.steps_per_beat() {
                            grid.steps_per_bar() - grid.steps_per_beat()
                        } else {
                            grid.steps_per_bar()
                        };
                        assert_eq!(last.step + last.length, limit);
                    }
                }
            }
        }
    }

    #[test]
    fn density_subdivides_gestures_without_removing_the_held_target() {
        let grid = Grid::default();
        let mut low_count = 0;
        let mut high_count = 0;
        for seed in 0..32 {
            let low = grouped(grid, 0.1, 0.5, None, &mut Rng::stream(seed, &[]));
            let high = grouped(grid, 0.9, 0.5, None, &mut Rng::stream(seed, &[]));
            low_count += low.len();
            high_count += high.len();
            assert!(high.iter().any(|cell| cell.length >= grid.steps_per_beat()));
        }
        assert!(high_count > low_count);
    }
}
