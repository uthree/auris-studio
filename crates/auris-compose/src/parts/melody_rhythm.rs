//! Rhythmic cells for the melody: a brief gesture and a held target share two felt beats.

use super::{Accent, Cell, Grid, Motif, PerformanceStyle, Rng};

/// Fill a short run with evenly spaced subdivisions, retaining a longer target beside it.
fn subdivisions(start: usize, length: usize, count: usize, onsets: &mut Vec<usize>) {
    for index in 0..count.min(length).max(1) {
        onsets.push(start + index * length / count.min(length).max(1));
    }
}

/// Write a bar from two-beat cells. Density divides a gesture; it never scatters isolated hits.
pub(super) fn grouped(
    grid: Grid,
    density: f32,
    syncopation: f32,
    style: Option<PerformanceStyle>,
    rng: &mut Rng,
) -> Vec<Cell> {
    let steps = grid.steps_per_bar();
    let beat = grid.steps_per_beat().min(steps).max(1);
    let density = density.clamp(0.0, 1.0);
    let syncopation = syncopation.clamp(0.0, 1.0);
    let splits = (1 + (density * 3.0).round() as usize).min(beat);
    // Held, short-long, long-short, anticipation, pickup. Styles choose gestures, while the
    // meter supplies their size; a dotted quarter in 6/8 remains a whole felt beat.
    let weights = match style {
        Some(PerformanceStyle::Ambient) => [8.0, 1.0, 1.0, 0.5, 0.5],
        Some(PerformanceStyle::Orchestral) => [4.0, 2.0, 2.0, 0.5, 0.5],
        Some(PerformanceStyle::JazzTrio | PerformanceStyle::CityPop) => {
            [0.5, 2.0, 2.0, 2.0 + 3.0 * syncopation, 2.0]
        }
        Some(PerformanceStyle::Rock | PerformanceStyle::Chiptune) => {
            [0.5, 4.0, 3.0, 0.5 + syncopation, 1.0]
        }
        _ => [0.5, 3.0, 2.0, 1.0 + 3.0 * syncopation, 1.0],
    };
    let mut cells = Vec::new();
    for start in (0..steps).step_by(2 * beat) {
        let end = (start + 2 * beat).min(steps);
        let length = end - start;
        let mut onsets = Vec::new();
        let mut target = start;
        if length <= beat {
            // An unmatched beat in an odd meter finishes the cell with one held arrival.
            onsets.push(start);
        } else {
            let middle = start + beat;
            let short = (beat / 2).max(1);
            match rng.weighted(&weights) {
                0 => onsets.push(start),
                1 => {
                    subdivisions(start, beat, splits, &mut onsets);
                    onsets.push(middle);
                    target = middle;
                }
                2 => {
                    onsets.push(start);
                    subdivisions(middle, end - middle, splits, &mut onsets);
                }
                3 if beat > 1 => {
                    target = middle - short;
                    subdivisions(start, target - start, splits, &mut onsets);
                    onsets.push(target);
                }
                _ => {
                    let pickup = start + short.min(beat - 1);
                    subdivisions(pickup, middle - pickup, splits, &mut onsets);
                    onsets.push(middle);
                    target = middle;
                }
            }
        }
        // Move a short preparation off the beat, keeping the held target intact. Changing
        // only palette weights can select the same rhythm at both ends of the syncopation dial.
        if beat > 1 && rng.chance(syncopation) {
            for position in 0..onsets.len() {
                let step = onsets[position];
                let next = onsets.get(position + 1).copied().unwrap_or(end);
                let held = step == target;
                let room = next - step;
                if step.is_multiple_of(beat) && room > if held { beat } else { 1 } {
                    let delay = (beat / 2).max(1).min(room - if held { beat } else { 1 });
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
    cells
}

/// Place a held arrival before the final felt beat, which is reserved for the next phrase.
pub(super) fn close_phrase(grid: Grid, figure: &mut Motif) {
    let steps = grid.steps_per_bar();
    let beat = grid.steps_per_beat();
    if steps <= beat || figure.cells.is_empty() {
        return;
    }
    let arrival_degree = figure.cells.last().map_or(0, |cell| cell.degree);
    let release = steps - beat;
    let target = release.saturating_sub(beat);
    if let Some(index) = figure.cells.iter().rposition(|cell| cell.step <= target) {
        figure.cells.truncate(index + 1);
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
                        close_phrase(grid, &mut figure);
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
