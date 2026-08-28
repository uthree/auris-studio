//! Prints the design-guide measurements for every shipped preset.
//!
//! ```text
//! cargo run -p auris-compose --example measure
//! ```
//!
//! One row per preset and part: Longuet-Higgins–Lee syncopation per bar for the kit, pitch-class
//! entropy and interval grammar for the tune. Averaged over a handful of seeds, because one seed
//! is one draw. Swing is forced straight before composing so the numbers describe the *pattern*
//! the composer chose rather than where the feel later nudged it.
//!
//! This is a reading instrument, not a test: the numbers move when a dial or a writer changes,
//! and whether a move is good is a judgement — Witek's inverted U says the middle syncopation is
//! where the pleasure sits, not the top. Run it before and after a change and read the diff.

use auris_compose::rhythm::{Accent, Grid, Pattern};
use auris_compose::spec::Role;
use auris_compose::{PRESETS, compose, metrics};

fn main() {
    println!(
        "{:<12} {:<9} {:>7} {:>9} {:>8} {:>8}",
        "preset", "part", "sync/bar", "pc-bits", "steps%", "mean-int"
    );
    for preset in PRESETS {
        let base = preset.spec();
        let seeds = [base.seed, 101, 102, 103, 104, 105, 106, 107];
        let grid = Grid::new(base.meter, 4);
        let step = grid.step_ticks().raw().max(1);

        // name → role, off the roster; every track came from a part.
        let role_of = |name: &str| {
            base.parts
                .iter()
                .find(|part| part.name == name)
                .map(|part| part.role)
        };

        // (part name) → summed metrics over the seeds, in roster order.
        let mut rows: Vec<(String, Vec<f64>, usize)> = Vec::new();
        for seed in seeds {
            let mut spec = base.clone();
            spec.seed = seed;
            // The pattern, not the feel: swung notes quantised back would smear into the
            // neighbouring step and the measure would read the smear.
            spec.swing = 50;
            let piece = compose(&spec);
            for track in &piece.tracks {
                let Some(role) = role_of(&track.name) else {
                    continue;
                };
                let notes: Vec<auris_core::Note> = track
                    .clips
                    .iter()
                    .flat_map(|clip| {
                        clip.notes.iter().map(|note| auris_core::Note {
                            velocity: note.velocity,
                            ..auris_core::Note::new(
                                note.pitch,
                                clip.start + note.start,
                                note.length,
                            )
                        })
                    })
                    .collect();
                let values = if role.is_drum() {
                    // The part as one long pattern: onset per nearest step, the constant lean
                    // rounded away.
                    let last = notes
                        .iter()
                        .map(|note| (note.start.raw() + step / 2) / step)
                        .max()
                        .unwrap_or(0) as usize;
                    let mut pattern = Pattern::rests(
                        (last + 1).div_ceil(grid.steps_per_bar()) * grid.steps_per_bar(),
                    );
                    for note in &notes {
                        let at = ((note.start.raw() + step / 2) / step) as usize;
                        pattern.steps[at] = Some(Accent::Normal);
                    }
                    vec![
                        metrics::syncopation(&pattern, grid),
                        f64::NAN,
                        f64::NAN,
                        f64::NAN,
                    ]
                } else if role == Role::Melody {
                    let line: Vec<i32> = notes.iter().map(|note| i32::from(note.pitch)).collect();
                    let steps = line
                        .windows(2)
                        .filter(|pair| (pair[0] - pair[1]).abs() <= 2 && pair[0] != pair[1])
                        .count();
                    let moves = line.windows(2).count().max(1);
                    let mean = line
                        .windows(2)
                        .map(|pair| f64::from((pair[0] - pair[1]).abs()))
                        .sum::<f64>()
                        / moves as f64;
                    vec![
                        f64::NAN,
                        metrics::pitch_class_entropy(&notes),
                        steps as f64 / moves as f64 * 100.0,
                        mean,
                    ]
                } else {
                    continue;
                };
                match rows.iter_mut().find(|(name, ..)| *name == track.name) {
                    Some((_, sums, count)) => {
                        for (sum, value) in sums.iter_mut().zip(&values) {
                            *sum += value;
                        }
                        *count += 1;
                    }
                    None => rows.push((track.name.clone(), values, 1)),
                }
            }
        }

        for (name, sums, count) in rows {
            let mean = |value: f64| value / count as f64;
            let cell = |value: f64, width: usize| {
                if value.is_nan() {
                    format!("{:>width$}", "·")
                } else {
                    format!("{:>width$.2}", mean(value))
                }
            };
            println!(
                "{:<12} {:<9} {} {} {} {}",
                preset.name,
                name,
                cell(sums[0], 8),
                cell(sums[1], 9),
                cell(sums[2], 8),
                cell(sums[3], 8),
            );
        }
    }
}
