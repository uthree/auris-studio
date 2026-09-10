//! A recurring rhythmic answer to the opening bar, written into the editable score.

use auris_core::time::Ticks;

use super::part_grid;
use super::{
    Draft, Frame, Grid, PartSpec, PerformanceStyle, Rng, RngKey, ScoreSettings, SectionPlan,
};

/// Let the echo answer the call with a delayed entry or an anticipated late arrival.
///
/// These moves retain pitch, velocity, note order and the end of the gesture. They only
/// weaken metric positions within the same harmonic event, so the pitch writer's prepared
/// resolutions remain ordered and no note is moved onto an unprepared stronger beat.
pub(super) fn connect(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    part: &PartSpec,
    notes: &mut [Draft],
) {
    if part.rhythm.is_some()
        || matches!(
            settings.style,
            Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral)
        )
    {
        return;
    }
    let grid = part_grid(frame, part);
    let beat = grid.steps_per_beat();
    let division = if grid.signature.is_compound() || grid.is_triplet() {
        3
    } else {
        2
    };
    if !beat.is_multiple_of(division) || beat < division {
        return;
    }
    let shift = grid.tick_of(beat / division);
    let mut rng = Rng::stream(
        frame.seed,
        &[
            RngKey::Word("part"),
            RngKey::Word(&part.name),
            RngKey::Word("melody-flow"),
            RngKey::Word(&section.name),
        ],
    );
    // Repeated phrases use the same answer, including later instances of the section.
    let pickup_first = rng.chance(0.5);
    for phrase in &section.phrases {
        if phrase.bars < 3 {
            continue;
        }
        let start = section.start + grid.bar_ticks() * (phrase.start_bar + 1) as i64;
        let end = start + grid.bar_ticks();
        let first = notes.partition_point(|note| note.start < start);
        let last = notes.partition_point(|note| note.start < end);
        let echo = &mut notes[first..last];
        if echo.len() < 3 {
            continue;
        }
        if pickup_first {
            if !delay_head(grid, section, start, shift, echo) {
                anticipate_tail(grid, section, start, shift, echo);
            }
        } else if !anticipate_tail(grid, section, start, shift, echo) {
            delay_head(grid, section, start, shift, echo);
        }
    }
}

fn same_harmony(section: &SectionPlan, before: Ticks, after: Ticks) -> bool {
    match (
        section.chord_at(before - section.start),
        section.chord_at(after - section.start),
    ) {
        (Some(a), Some(b)) => a.start == b.start,
        _ => false,
    }
}

fn delay_head(
    grid: Grid,
    section: &SectionPlan,
    start: Ticks,
    shift: Ticks,
    notes: &mut [Draft],
) -> bool {
    let Some(first) = notes.first_mut() else {
        return false;
    };
    if first.start != start
        || first.length < grid.tick_of(grid.steps_per_beat())
        || first.length <= shift
        || !same_harmony(section, first.start, first.start + shift)
    {
        return false;
    }
    first.start += shift;
    first.length -= shift;
    true
}

fn anticipate_tail(
    grid: Grid,
    section: &SectionPlan,
    start: Ticks,
    shift: Ticks,
    notes: &mut [Draft],
) -> bool {
    let beat = grid.tick_of(grid.steps_per_beat());
    // A shorter bar would move this long note back into its middle.
    if notes.len() < 2 || grid.bar_ticks() < beat * 4 {
        return false;
    }
    let last = notes.len() - 1;
    let before = notes[last - 1];
    let target = notes[last];
    if target.start != start + grid.bar_ticks() - beat
        || target.length < beat
        || before.start + before.length != target.start
        || before.length < shift * 2
        || !same_harmony(section, target.start, target.start - shift)
        || grid.weight(grid.step_of(target.start - shift - start))
            > grid.weight(grid.step_of(target.start - start))
    {
        return false;
    }
    notes[last - 1].length -= shift;
    notes[last].start -= shift;
    notes[last].length += shift;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::fixture::{BASE, draft};

    fn note(at: f64, length: f64, pitch: u8) -> Draft {
        Draft {
            section: 0,
            start: Ticks::from_beats(at),
            length: Ticks::from_beats(length),
            pitch,
            velocity: 0.73,
        }
    }

    #[test]
    fn a_delayed_echo_keeps_its_pitch_and_endpoint() {
        let (_, frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let mut notes = [note(4.0, 1.0, 67), note(5.0, 1.0, 69), note(6.0, 2.0, 71)];
        let before = notes;
        assert!(delay_head(
            frame.grid,
            &frame.sections[0],
            Ticks::from_beats(4.0),
            Ticks::from_beats(0.5),
            &mut notes,
        ));
        assert_eq!(notes[0], note(4.5, 0.5, 67));
        assert_eq!(notes[1..], before[1..]);
    }

    #[test]
    fn a_late_arrival_is_anticipated_without_inserting_notes_or_a_central_hold() {
        let (_, frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let mut notes = [note(4.0, 2.0, 67), note(6.0, 1.0, 69), note(7.0, 1.0, 71)];
        assert!(anticipate_tail(
            frame.grid,
            &frame.sections[0],
            Ticks::from_beats(4.0),
            Ticks::from_beats(0.5),
            &mut notes,
        ));
        assert_eq!(
            notes,
            [note(4.0, 2.0, 67), note(6.0, 0.5, 69), note(6.5, 1.5, 71)]
        );
    }

    #[test]
    fn pickups_and_existing_late_holds_keep_their_timing() {
        let (_, frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let mut notes = [note(4.5, 0.5, 67), note(5.0, 1.5, 69), note(6.5, 1.5, 71)];
        let before = notes;
        let start = Ticks::from_beats(4.0);
        let shift = Ticks::from_beats(0.5);
        assert!(!delay_head(
            frame.grid,
            &frame.sections[0],
            start,
            shift,
            &mut notes
        ));
        assert!(!anticipate_tail(
            frame.grid,
            &frame.sections[0],
            start,
            shift,
            &mut notes
        ));
        assert_eq!(notes, before);
    }

    #[test]
    fn an_anticipation_does_not_cross_a_chord_change_or_erase_a_rest() {
        let (_, mut frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let section = &mut frame.sections[0];
        section.events[1].start = Ticks::from_beats(7.0);
        section.events[0].length = Ticks::from_beats(7.0);
        let mut notes = [note(4.0, 2.0, 67), note(6.0, 1.0, 69), note(7.0, 1.0, 71)];
        let before = notes;
        assert!(!anticipate_tail(
            frame.grid,
            section,
            Ticks::from_beats(4.0),
            Ticks::from_beats(0.5),
            &mut notes
        ));
        assert_eq!(notes, before);
        section.events[1].start = Ticks::from_beats(4.0);
        section.events[0].length = Ticks::from_beats(4.0);
        notes[1].length = Ticks::from_beats(0.5);
        let with_rest = notes;
        assert!(!anticipate_tail(
            frame.grid,
            section,
            Ticks::from_beats(4.0),
            Ticks::from_beats(0.5),
            &mut notes
        ));
        assert_eq!(notes, with_rest);
    }

    #[test]
    fn only_the_echo_changes_and_every_pitch_and_velocity_survives() {
        let (spec, frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let mut notes: Vec<_> = (0..16)
            .map(|at| note(f64::from(at), 1.0, 60 + (at % 7) as u8))
            .collect();
        let before = notes.clone();
        connect(
            &ScoreSettings::from(&spec),
            &frame,
            &frame.sections[0],
            &spec.parts[0],
            &mut notes,
        );
        assert_eq!(notes[..4], before[..4]);
        assert_ne!(notes[4..8], before[4..8]);
        assert_eq!(notes[8..], before[8..]);
        assert_eq!(notes.len(), before.len());
        for (after, before) in notes.iter().zip(&before) {
            assert_eq!(
                (after.pitch, after.velocity),
                (before.pitch, before.velocity)
            );
            assert!(after.length > Ticks::ZERO);
        }
        assert!(
            notes
                .windows(2)
                .all(|pair| pair[0].start + pair[0].length <= pair[1].start)
        );
    }

    #[test]
    fn authored_rhythms_and_sustained_styles_are_unchanged() {
        let (spec, frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let original: Vec<_> = (0..16).map(|at| note(f64::from(at), 1.0, 67)).collect();
        for style in [PerformanceStyle::Ambient, PerformanceStyle::Orchestral] {
            let mut settings = ScoreSettings::from(&spec);
            settings.style = Some(style);
            let mut notes = original.clone();
            connect(
                &settings,
                &frame,
                &frame.sections[0],
                &spec.parts[0],
                &mut notes,
            );
            assert_eq!(notes, original);
        }
        let mut part = spec.parts[0].clone();
        part.rhythm = Some(crate::rhythm::Pattern::parse("x...x...x...x...").unwrap());
        let mut notes = original.clone();
        connect(
            &ScoreSettings::from(&spec),
            &frame,
            &frame.sections[0],
            &part,
            &mut notes,
        );
        assert_eq!(notes, original);
    }

    #[test]
    fn short_meters_do_not_move_a_late_hold_into_the_middle() {
        let (_, frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let grid = Grid::new(auris_core::time::TimeSignature::new(3, 4), 4);
        let mut notes = [note(0.0, 1.0, 67), note(1.0, 1.0, 69), note(2.0, 1.0, 71)];
        let before = notes;
        assert!(!anticipate_tail(
            grid,
            &frame.sections[0],
            Ticks::ZERO,
            Ticks::from_beats(0.5),
            &mut notes
        ));
        assert_eq!(notes, before);
    }

    #[test]
    fn a_delayed_entry_cannot_cross_a_short_chord() {
        let (_, mut frame, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        let section = &mut frame.sections[0];
        section.events[0].length = Ticks::from_beats(0.25);
        section.events[1].start = Ticks::from_beats(0.25);
        let mut notes = [note(0.0, 1.0, 67), note(1.0, 1.0, 69), note(2.0, 2.0, 71)];
        let before = notes;
        assert!(!delay_head(
            frame.grid,
            section,
            Ticks::ZERO,
            Ticks::from_beats(0.5),
            &mut notes
        ));
        assert_eq!(notes, before);
    }

    #[test]
    fn echo_moves_respect_phrase_offsets_and_the_meters_own_subdivisions() {
        use auris_core::project::Subdivision;
        use auris_core::time::TimeSignature;

        let (spec, template, _) = draft(&format!("{BASE}\n[[part]]\nname = \"lead\""));
        for signature in [
            TimeSignature::new(4, 4),
            TimeSignature::new(3, 4),
            TimeSignature::new(6, 8),
            TimeSignature::new(12, 8),
        ] {
            for subdivision in Subdivision::ALL {
                for bars in 1..=8 {
                    let mut frame = template.clone();
                    let grid = Grid::new(signature, subdivision.steps_per_beat());
                    frame.grid = grid;
                    let mut part = spec.parts[0].clone();
                    part.subdivision = subdivision;
                    let mut section = frame.sections[0].clone();
                    section.start = Ticks::from_beats(8.0);
                    section.bars = bars;
                    section.length = grid.bar_ticks() * bars as i64;
                    section.phrases = crate::phrasing::plan_phrases(bars, None);
                    section.events.truncate(1);
                    section.events[0].length = section.length;
                    let beat = grid.steps_per_beat();
                    let mut slots: Vec<_> = (0..grid.steps_per_bar()).step_by(beat).collect();
                    if slots.len() == 2 {
                        slots.push(beat + beat / 3);
                    }
                    let mut notes = Vec::new();
                    for bar in 0..bars {
                        for (index, step) in slots.iter().enumerate() {
                            let end = slots
                                .get(index + 1)
                                .copied()
                                .unwrap_or(grid.steps_per_bar());
                            notes.push(Draft {
                                start: section.start
                                    + grid.bar_ticks() * bar as i64
                                    + grid.tick_of(*step),
                                length: grid.tick_of(end - step),
                                ..note(0.0, 1.0, 67)
                            });
                        }
                    }
                    let before = notes.clone();
                    connect(
                        &ScoreSettings::from(&spec),
                        &frame,
                        &section,
                        &part,
                        &mut notes,
                    );
                    let division = if signature.is_compound() || grid.is_triplet() {
                        3
                    } else {
                        2
                    };
                    if bars < 3 || !beat.is_multiple_of(division) || beat < division {
                        assert_eq!(notes, before);
                    } else {
                        assert_ne!(notes, before, "{signature:?}/{subdivision:?}/{bars}");
                    }
                    assert_eq!(notes.len(), before.len());
                    for (after, old) in notes.iter().zip(&before) {
                        assert_eq!((after.pitch, after.velocity), (old.pitch, old.velocity));
                        assert!(after.length > Ticks::ZERO);
                        let bar =
                            ((old.start - section.start).raw() / grid.bar_ticks().raw()) as usize;
                        let offset = section.start + grid.bar_ticks() * bar as i64;
                        assert!(
                            after.start >= offset
                                && after.start + after.length <= offset + grid.bar_ticks()
                        );
                        assert!(same_harmony(&section, old.start, after.start));
                        assert!(
                            grid.weight(grid.step_of(after.start - offset))
                                <= grid.weight(grid.step_of(old.start - offset))
                        );
                        if after != old {
                            assert!(
                                section
                                    .phrases
                                    .iter()
                                    .any(|phrase| phrase.bars >= 3 && bar == phrase.start_bar + 1)
                            );
                        }
                    }
                    assert!(
                        notes
                            .windows(2)
                            .all(|pair| pair[0].start + pair[0].length <= pair[1].start)
                    );
                }
            }
        }
    }
}
