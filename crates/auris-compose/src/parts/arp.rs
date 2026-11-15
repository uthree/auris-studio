//! A broken chord.
//!
//! The shortest of the writers, and its own file for the same reason as the rest: nothing calls
//! it. Two things worth knowing before reading it. Its density is a *rate* — how fast the figure
//! runs — rather than how many of its notes survive, because an arpeggio with holes punched in it
//! is not a sparser arpeggio, it is a broken one. And the notes it breaks are not its own: they
//! are the inversion `voicing` leads from one chord to the next, by the rule the comp's chords
//! are led by, so the figure moves under a chord change the way a keyboard player's hand does.

use crate::PerformanceStyle;
use crate::frame::{Frame, SectionPlan};
use crate::rng::{Key as RngKey, Rng};
use crate::spec::PartSpec;
use crate::theory::pitch::OCTAVE;

use super::voicing::voiced_closed;
use super::writer::{density, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// The order an arpeggio visits a chord's notes in.
///
/// Up, down and each of them turning round were the whole of it, so every arpeggio was a scale
/// of chord tones run one way or the other. The last two are the figures a keyboard player
/// reaches for when the chord is to be heard as a chord rather than as a run.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(super) enum ArpShape {
    /// Bottom to top, and again.
    Up,
    /// Top to bottom, and again.
    Down,
    /// Up and back down, without repeating the note it turns on.
    UpDown,
    /// Down and back up, the same way.
    DownUp,
    /// Every other note up, then the ones between: 1-5-3 of a triad, 1-5-3-8 of four notes.
    Broken,
    /// Each note in turn against the top one: low, high, middle, high — the Alberti bass.
    Alberti,
}

impl ArpShape {
    const ALL: [ArpShape; 6] = [
        ArpShape::Up,
        ArpShape::Down,
        ArpShape::UpDown,
        ArpShape::DownUp,
        ArpShape::Broken,
        ArpShape::Alberti,
    ];

    /// The indices of a voicing of `count` notes, lowest first, in the order this shape plays
    /// them; the figure is this list round and round.
    fn order(self, count: usize) -> Vec<usize> {
        let up = || 0..count;
        // A chord of two notes has nothing to turn on or to skip: every shape is up or down.
        let inner = || 1..count.saturating_sub(1);
        match self {
            ArpShape::Up => up().collect(),
            ArpShape::Down => up().rev().collect(),
            ArpShape::UpDown => up().chain(inner().rev()).collect(),
            ArpShape::DownUp => up().rev().chain(inner()).collect(),
            ArpShape::Broken => up().step_by(2).chain(up().skip(1).step_by(2)).collect(),
            ArpShape::Alberti if count > 2 => (0..count - 1)
                .flat_map(|index| [index, count - 1])
                .collect(),
            ArpShape::Alberti => up().collect(),
        }
    }
}

/// Draws the shape a section's arpeggio runs in.
///
/// Every shape keeps some weight in every idiom, for the reason the comp's figures do: a table
/// that forbids a choice outright makes every take the same again.
fn pick_shape(rng: &mut Rng, style: Option<PerformanceStyle>) -> ArpShape {
    let mut weights = [1.0, 0.4, 0.8, 0.3, 0.7, 0.6];
    let vocabulary = match style {
        // A sequencer's arpeggio is a run.
        Some(PerformanceStyle::Synthwave | PerformanceStyle::Chiptune) => {
            [2.0, 1.5, 0.8, 0.6, 1.0, 0.3]
        }
        // A harp's, or a held pedal's, rises and falls.
        Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral) => {
            [1.0, 0.6, 2.5, 1.5, 0.6, 0.3]
        }
        // A band's keyboard or guitar picks the chord apart.
        Some(PerformanceStyle::PopBand | PerformanceStyle::CityPop | PerformanceStyle::Rock) => {
            [0.8, 0.4, 0.8, 0.4, 1.8, 1.8]
        }
        Some(PerformanceStyle::JazzTrio) => [0.8, 0.6, 1.4, 1.0, 1.6, 0.4],
        None => [1.0; 6],
    };
    for (weight, preference) in weights.iter_mut().zip(vocabulary) {
        *weight *= preference;
    }
    ArpShape::ALL[rng.weighted(&weights).min(ArpShape::ALL.len() - 1)]
}

/// A broken chord.
pub(super) fn arp(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let (low, high) = part.range();
    let grid = part_grid(frame, part);
    // How fast the figure runs. An arpeggio's density is the rate it climbs at, not how many of
    // its notes are dropped — dropping them would leave a broken chord with holes in it.
    let busy = density(settings, part, section);
    let step_length = grid.step_ticks()
        * if busy > 0.66 {
            1
        } else if busy > 0.33 {
            2
        } else {
            4
        };
    let mut notes = Vec::new();
    let mut rng = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("arp"),
            // Not the instance: a repeat of a section runs its arpeggio the same way round.
            RngKey::Word(&section.name),
        ],
    );
    let shape = pick_shape(&mut rng, settings.style);
    // How many octaves the figure covers. The sustained palettes have the room for two.
    let wide = match settings.style {
        Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral) => 0.75,
        _ => 0.5,
    };
    let span = if rng.chance(wide) { 2 } else { 1 };
    // Where the chord sits: the middle of the part's range, and half an octave under it when
    // the figure carries the chord up an octave, so the two together straddle the same middle.
    let centre = (low + high) / 2 - if span > 1 { OCTAVE / 2 } else { 0 };
    // The chord before this one, as it was voiced, for the next to be led from. This is the
    // difference between an arpeggio and a chord stacked from its root every time: stacked,
    // the figure leapt nine semitones at a chord change, four in ten of them more than a fifth,
    // while the comp beside it crossed the same change in one or two.
    let mut previous: Vec<i32> = Vec::new();

    for event in &section.events {
        let chord = voiced_closed(event.chord, &previous, (low, high), centre, Some(&mut rng));
        previous.clone_from(&chord);
        let mut voicing = chord.clone();
        for octave in 1..span {
            voicing.extend(
                chord
                    .iter()
                    .map(|pitch| pitch + octave * OCTAVE)
                    .filter(|pitch| *pitch <= high),
            );
        }
        voicing.sort_unstable();
        voicing.dedup();
        if voicing.is_empty() {
            continue;
        }
        let order = shape.order(voicing.len());
        let figure = |strike: usize| voicing[order[strike % order.len()]];
        let draft = |at: auris_core::time::Ticks, pitch: i32, length| Draft {
            section: index,
            pitch: pitch.clamp(0, 127) as u8,
            velocity: (velocity(
                grid.weight(grid.step_of(at)),
                section.intensity,
                settings.dynamics,
            ) * 0.8
                * phrase_shape(grid, section, at, settings.dynamics))
            .clamp(0.05, 1.0),
            start: section.start + at,
            length,
        };
        // A rhythm the user wrote is played as written: the figure strikes on those steps and
        // only those, in the same order it always runs.
        if let Some(pattern) = part.rhythm.as_ref() {
            let per_bar = grid.steps_per_bar().max(1);
            let from = grid.step_of(event.start);
            let mut strike = 0usize;
            for offset in 0..grid.step_of(event.length) {
                if pattern.at((from + offset) % per_bar).is_none() {
                    continue;
                }
                let at = event.start + grid.tick_of(offset);
                notes.push(draft(at, figure(strike), grid.step_ticks()));
                strike += 1;
            }
            continue;
        }

        // Even a chord shorter than the chosen rate needs an onset: otherwise a coarse
        // arpeggio silently skips dense harmony changes altogether.
        let count = (event.length.raw() / step_length.raw().max(1)).max(1) as usize;
        for position in 0..count {
            let at = event.start + step_length * position as i64;
            notes.push(draft(at, figure(position), step_length));
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Song, part};
    use crate::spec::Role;

    /// The arpeggio alone over `chart` for eight bars at `seed`.
    fn arpeggio(chart: &str, seed: u64) -> Song {
        Song {
            chords: chart.to_string(),
            bars: 8,
            seed,
            ..Song::with_parts([PartSpec::of_role("arp", Role::Arp)])
        }
    }

    #[test]
    fn every_shape_visits_every_note_of_the_chord() {
        assert_eq!(ArpShape::Up.order(3), [0, 1, 2]);
        assert_eq!(ArpShape::Down.order(3), [2, 1, 0]);
        assert_eq!(ArpShape::UpDown.order(4), [0, 1, 2, 3, 2, 1]);
        assert_eq!(ArpShape::DownUp.order(4), [3, 2, 1, 0, 1, 2]);
        assert_eq!(ArpShape::Broken.order(3), [0, 2, 1], "1-5-3");
        assert_eq!(ArpShape::Broken.order(4), [0, 2, 1, 3], "1-5-3-8");
        assert_eq!(
            ArpShape::Alberti.order(3),
            [0, 2, 1, 2],
            "low, high, middle, high"
        );
        assert_eq!(ArpShape::Alberti.order(4), [0, 3, 1, 3, 2, 3]);
        for shape in ArpShape::ALL {
            for count in 1..=8 {
                let order = shape.order(count);
                assert!(!order.is_empty(), "{shape:?} of {count} plays nothing");
                for index in 0..count {
                    assert!(
                        order.contains(&index),
                        "{shape:?} of {count} never plays note {index}: {order:?}"
                    );
                }
                assert!(order.iter().all(|index| *index < count));
                // Nothing is struck twice running, round the corner included: a figure that
                // repeats a note is a figure with a stutter in it.
                if count > 1 {
                    for (position, index) in order.iter().enumerate() {
                        assert_ne!(
                            *index,
                            order[(position + 1) % order.len()],
                            "{shape:?} of {count} repeats a note: {order:?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn an_arpeggio_is_led_from_chord_to_chord_like_the_comp_beside_it() {
        // Stacked from its root every time, the figure's lowest note moved with the root: a
        // fourth or a fifth at every change of the commonest progressions. Led, the hand stays
        // where it is and the notes under it change.
        for chart in ["@axis", "@royal-road", "@canon", "| I | IV | V | I |"] {
            for seed in 0..16u64 {
                let (frame, parts) = arpeggio(chart, seed).draft();
                let arp = part(&parts, "arp");
                let section = &frame.sections[0];
                let floors: Vec<i32> = section
                    .events
                    .iter()
                    .map(|event| {
                        arp.notes
                            .iter()
                            .filter(|note| {
                                note.start >= section.start + event.start
                                    && note.start < section.start + event.end()
                            })
                            .map(|note| i32::from(note.pitch))
                            .min()
                            .expect("every chord is arpeggiated")
                    })
                    .collect();
                let moved: i32 = floors
                    .windows(2)
                    .map(|pair| (pair[1] - pair[0]).abs())
                    .sum();
                let changes = (floors.len() - 1) as f32;
                assert!(
                    moved as f32 / changes <= 3.0,
                    "{chart}, seed {seed}: the figure's floor moves {:.1} semitones a change",
                    moved as f32 / changes
                );
            }
        }
    }

    #[test]
    fn an_arpeggio_plays_its_chord_inside_its_range_at_one_rate() {
        let (low, high) = Role::Arp.range();
        for chart in ["@axis", "@marusa", "@koakuma", "@blues"] {
            for seed in 0..8u64 {
                let (frame, parts) = arpeggio(chart, seed).draft();
                let arp = part(&parts, "arp");
                let section = &frame.sections[0];
                assert!(!arp.notes.is_empty());
                for note in &arp.notes {
                    let event = section
                        .chord_at(note.start - section.start)
                        .expect("a chord under every note");
                    assert!(
                        event.chord.contains_midi(i32::from(note.pitch)),
                        "{chart}, seed {seed}: {} is not in {}",
                        note.pitch,
                        event.chord
                    );
                    assert!(
                        (low..=high).contains(&i32::from(note.pitch)),
                        "{chart}, seed {seed}: {} is outside {low}..{high}",
                        note.pitch
                    );
                }
                for pair in arp.notes.windows(2) {
                    assert_eq!(
                        pair[1].start - pair[0].start,
                        pair[0].length,
                        "{chart}, seed {seed}: the figure has a hole or an overlap in it"
                    );
                }
            }
        }
    }

    #[test]
    fn another_take_runs_another_shape() {
        // Up or down, turning or not, was every arpeggio there was.
        let shapes = |style| {
            let mut seen = Vec::new();
            for seed in 0..64u64 {
                let shape = pick_shape(&mut Rng::stream(seed, &[]), style);
                if !seen.contains(&shape) {
                    seen.push(shape);
                }
            }
            seen.len()
        };
        assert_eq!(shapes(None), ArpShape::ALL.len());
        assert_eq!(
            shapes(Some(PerformanceStyle::Synthwave)),
            ArpShape::ALL.len()
        );

        let count = |style, shape| {
            (0..512)
                .filter(|seed| pick_shape(&mut Rng::stream(*seed, &[]), style) == shape)
                .count()
        };
        assert!(
            count(Some(PerformanceStyle::Synthwave), ArpShape::Up)
                > count(Some(PerformanceStyle::PopBand), ArpShape::Up) * 2
        );
        assert!(
            count(Some(PerformanceStyle::PopBand), ArpShape::Alberti)
                > count(Some(PerformanceStyle::Synthwave), ArpShape::Alberti) * 3
        );
    }
}
