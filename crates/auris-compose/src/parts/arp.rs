//! A broken chord.
//!
//! The shortest of the writers, and its own file for the same reason as the rest: nothing calls
//! it. Two things worth knowing before reading it. Its density is a *rate* — how fast the figure
//! runs, beat by beat — rather than how many of its notes survive, because an arpeggio with
//! holes punched in it is not a sparser arpeggio, it is a broken one. And the notes it breaks
//! are not its own: they
//! are the inversion `voicing` leads from one chord to the next, by the rule the comp's chords
//! are led by, so the figure moves under a chord change the way a keyboard player's hand does.

use crate::PerformanceStyle;
use crate::frame::{Frame, SectionPlan};
use crate::rhythm::Grid;
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

/// The steps of a bar an arpeggio this busy strikes.
///
/// Each beat runs at one of three rates — once, at the beat's own division, or on every step
/// of the grid — and the dial turns the beats over one at a time: the weakest beat first, the
/// last of them when two are equally weak, the downbeat last. So a figure a little busier than
/// quarters is quarters with its last beat split, long long long short-short, and the one
/// after that splits the second beat too. A simple beat divides in two and a compound one in
/// three, which is the eighths of a 6/8 and not the dotted eighths that halving it would give.
///
/// It used to be one rate for the whole bar, chosen from three, and the dial did nothing
/// between them: every setting from a quarter of the way to a half wrote the same notes, and
/// the next notch doubled them. The three plateaus are still where they were — quarters at the
/// bottom, eighths round the middle, every step at the top — with a ramp between each pair.
fn strikes(grid: Grid, busy: f32) -> Vec<usize> {
    let per_bar = grid.steps_per_bar().max(1);
    let per_beat = grid.steps_per_beat().clamp(1, per_bar);
    let beats: Vec<usize> = (0..per_bar).step_by(per_beat).collect();
    // The order the beats turn over in: by how little the bar leans on them, latest first.
    let mut order: Vec<usize> = (0..beats.len()).collect();
    order.sort_by_key(|beat| (grid.weight(beats[*beat]), std::cmp::Reverse(*beat)));
    let turned = |from: f32| {
        let share = ((busy - from) / RAMP).clamp(0.0, 1.0);
        (share * beats.len() as f32).round() as usize
    };
    let (halved, run) = (turned(HALVES_FROM), turned(RUNS_FROM));
    let parts = if grid.signature.is_compound() { 3 } else { 2 };
    let mut steps = Vec::new();
    for (rank, beat) in order.iter().enumerate() {
        let start = beats[*beat];
        let end = (start + per_beat).min(per_bar);
        let every = if rank < run {
            1
        } else if rank < halved && per_beat.is_multiple_of(parts) {
            per_beat / parts
        } else if rank < halved {
            // A grid that does not divide the beat that way has no rate between: the next one
            // up is all of it.
            1
        } else {
            per_beat
        };
        steps.extend((start..end).step_by(every));
    }
    steps.sort_unstable();
    steps
}

/// Where the dial starts splitting beats in two, and where it starts running them on every
/// step. Each ramp is [`RAMP`] wide, which leaves the plateau between them — a bar of plain
/// eighths — a fifth of the dial, with the default in the middle of it.
const HALVES_FROM: f32 = 0.25;
/// See [`HALVES_FROM`].
const RUNS_FROM: f32 = 0.6;
/// See [`HALVES_FROM`].
const RAMP: f32 = 0.15;

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
    let struck = strikes(grid, busy);
    let mut notes: Vec<Draft> = Vec::new();
    // The notes the figure before this chord would have struck next, for the one place a
    // figure says a note twice: see below.
    let mut carried_on: Vec<i32> = Vec::new();
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
            // A rhythm somebody wrote is played as written, repeats and all.
            carried_on.clear();
            continue;
        }

        let per_bar = grid.steps_per_bar().max(1);
        let from = grid.step_of(event.start);
        let span = grid.step_of(event.length).max(1);
        let mut offsets: Vec<usize> = (0..span)
            .filter(|offset| struck.binary_search(&((from + offset) % per_bar)).is_ok())
            .collect();
        // Even a chord shorter than the chosen rate needs an onset: otherwise a coarse
        // arpeggio silently skips dense harmony changes altogether.
        if offsets.first() != Some(&0) {
            offsets.insert(0, 0);
        }
        // A figure restarts on every chord, and one bar line in twelve it restarted on the note
        // it had just struck: a run that ends on its bottom note and begins again there, or a
        // tone two chords share. Nothing inside a bar ever repeats a note, so at the line it
        // was a stutter. The note before the line moves on to where its own figure was going.
        let before = notes
            .len()
            .checked_sub(2)
            .map(|index| i32::from(notes[index].pitch));
        if let Some(last) = notes.last_mut()
            && i32::from(last.pitch) == figure(0)
            && let Some(onward) = carried_on
                .iter()
                .find(|pitch| **pitch != figure(0) && Some(**pitch) != before)
        {
            last.pitch = (*onward).clamp(0, 127) as u8;
        }
        for (position, offset) in offsets.iter().enumerate() {
            let here = grid.tick_of(*offset);
            let next = offsets
                .get(position + 1)
                .map_or(event.length, |next| grid.tick_of(*next));
            notes.push(draft(
                event.start + here,
                figure(position),
                (next - here).max(auris_core::time::Ticks(1)),
            ));
        }
        carried_on = (0..order.len())
            .map(|ahead| figure(offsets.len() + ahead))
            .collect();
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture::{Song, part};
    use crate::spec::Role;

    #[test]
    fn the_density_dial_turns_an_arpeggio_over_a_beat_at_a_time() {
        let grid = Grid::new(auris_core::time::TimeSignature::new(4, 4), 4);
        // The three rates it always had, where it always had them.
        assert_eq!(strikes(grid, 0.1), vec![0, 4, 8, 12]);
        assert_eq!(strikes(grid, 0.5), vec![0, 2, 4, 6, 8, 10, 12, 14]);
        assert_eq!(strikes(grid, 0.9), (0..16).collect::<Vec<_>>());
        // And between them a beat at a time, the last weak beat first: long long long
        // short-short, then the second beat as well.
        assert_eq!(strikes(grid, 0.30), vec![0, 4, 8, 12, 14]);
        assert_eq!(strikes(grid, 0.33), vec![0, 4, 6, 8, 12, 14]);
        let mut last = strikes(grid, 0.0).len();
        let mut biggest = 0;
        for hundredth in 0..=100 {
            let now = strikes(grid, hundredth as f32 / 100.0).len();
            assert!(
                now >= last,
                "asking for more took a note away at {hundredth}"
            );
            biggest = biggest.max(now - last);
            last = now;
        }
        // One beat's worth at the most. It used to double between 0.33 and 0.34.
        assert!(biggest <= 2, "the dial jumped by {biggest} notes");
        // A compound beat divides in three: the eighths of a 6/8, not its dotted eighths.
        let compound = Grid::new(auris_core::time::TimeSignature::new(6, 8), 4);
        assert_eq!(strikes(compound, 0.1), vec![0, 6]);
        assert_eq!(strikes(compound, 0.5), vec![0, 2, 4, 6, 8, 10]);
        assert_eq!(strikes(compound, 0.9).len(), compound.steps_per_bar());
        let waltz = Grid::new(auris_core::time::TimeSignature::new(3, 4), 4);
        assert_eq!(strikes(waltz, 0.5), vec![0, 2, 4, 6, 8, 10]);
    }

    #[test]
    fn an_arpeggio_never_strikes_the_note_it_has_just_struck() {
        // One bar line in twelve it did, at the rates where the figure's length leaves it on
        // its first note at the end of the bar.
        for chart in ["@axis", "| I | I | IV | IV |", "@canon"] {
            for density in [0.2, 0.3, 0.5, 0.7, 0.9] {
                for seed in 0..24u64 {
                    let mut song = arpeggio(chart, seed);
                    song.parts[0].density = Some(density);
                    let (_, parts) = song.draft();
                    let notes = &part(&parts, "arp").notes;
                    for pair in notes.windows(2) {
                        assert_ne!(
                            pair[0].pitch, pair[1].pitch,
                            "{chart} at {density}, seed {seed}: a note twice at {:?}",
                            pair[1].start
                        );
                    }
                }
            }
        }
    }

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
