//! Timing written into the editable score after the pitches are settled: the recurring answer
//! to a phrase's opening bar, and the places where the line crosses a bar line.

use auris_core::time::Ticks;

use super::melody_rhythm::offbeat_steps;
use super::{
    ChordScale, Draft, Frame, Grid, PartSpec, PerformanceStyle, Rng, RngKey, ScoreSettings,
    SectionPlan,
};
use super::{part_grid, phrase_shape, scale_shift, velocity};

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
    let Some(shift) = offbeat(grid) else {
        return;
    };
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

/// The meter's own offbeat in ticks. `None` on a grid too coarse to have one.
fn offbeat(grid: Grid) -> Option<Ticks> {
    offbeat_steps(grid).map(|steps| grid.tick_of(steps))
}

/// How often a section's answering bars come in ahead of their downbeat, at no syncopation and
/// at all of it.
///
/// A choice made once for the section, because an anticipation heard in one phrase and not
/// the next is a slip and one heard in every phrase is the figure. Measured over the PDMX pop
/// and rock vocal parts, three notes in sixteen bars are held across a bar line; the composer
/// wrote none at all, because a figure's cells begin and end inside their own bar.
const ANTICIPATES: (f32, f32) = (0.2, 0.8);

/// How often a phrase after the first is led into by a pickup.
///
/// Over the same corpus 37 lines in a hundred enter in the second half of a bar, and the
/// commonest entry point takes 63 of them. The composer's lines entered at one point 97 times
/// in a hundred, which is every phrase of a section walking through the same door.
const PICKUP: f32 = 0.45;

/// Where a section's line crosses its bar lines, decided before a note of it is written.
///
/// Decided first because a pickup needs room made for it: the phrase before has to let go a
/// beat sooner, and it can only do that if it knows while it is being closed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct Crossings {
    /// Whether the section's answering bars come in ahead of their downbeat.
    anticipates: bool,
    /// For each of the section's phrases, whether a pickup leads into it. Never the first.
    led_in: Vec<bool>,
}

impl Crossings {
    /// Draws the section's crossings from a stream of their own.
    ///
    /// Written rhythms keep every attack where it was typed and sustained palettes keep their
    /// stillness, as in [`connect`]: both get no crossings at all, and so does a grid with no
    /// offbeat to cross on.
    pub(super) fn plan(
        settings: &ScoreSettings,
        frame: &Frame,
        section: &SectionPlan,
        part: &PartSpec,
    ) -> Self {
        if part.rhythm.is_some()
            || matches!(
                settings.style,
                Some(PerformanceStyle::Ambient | PerformanceStyle::Orchestral)
            )
            || offbeat(part_grid(frame, part)).is_none()
        {
            return Self::default();
        }
        let mut rng = Rng::stream(
            frame.seed,
            &[
                RngKey::Word("part"),
                RngKey::Word(&part.name),
                RngKey::Word("melody-crossing"),
                RngKey::Word(&section.name),
            ],
        );
        let syncopation = settings.mood.syncopation.clamp(0.0, 1.0);
        let anticipates = rng.chance(ANTICIPATES.0 + (ANTICIPATES.1 - ANTICIPATES.0) * syncopation);
        let led_in = section
            .phrases
            .iter()
            .map(|phrase| {
                // Rolled for the first phrase too, which has no bar before it to lead in
                // from, so that every later phrase draws the same number whatever came first.
                let drawn = rng.chance(PICKUP);
                drawn && phrase.start_bar > 0
            })
            .collect();
        Self {
            anticipates,
            led_in,
        }
    }

    /// Whether the phrase after the one holding `bar` is led into by a pickup, which is what
    /// the phrase holding `bar` has to leave room for.
    pub(super) fn follow(&self, section: &SectionPlan, bar: usize) -> bool {
        section
            .phrases
            .iter()
            .position(|phrase| phrase.start_bar <= bar && bar < phrase.end_bar())
            .and_then(|index| self.led_in.get(index + 1))
            .copied()
            .unwrap_or(false)
    }
}

/// Let the line cross the bar lines the figure is written inside.
///
/// Two moves, and neither changes a pitch already written. An answering bar that enters on
/// its downbeat can come in an offbeat early and hold across the bar line: the anticipated
/// note is already a note of the chord it is arriving at, which is what an anticipation is.
/// And a phrase after the first can be led into by one note in the breath the phrase before
/// it left, a scale step from the downbeat it is going to, on the side the line is coming
/// from. A pickup is a new note and is the only thing here that adds one.
pub(super) fn cross_bar_lines(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    part: &PartSpec,
    crossings: &Crossings,
    notes: &mut Vec<Draft>,
) {
    let grid = part_grid(frame, part);
    let Some(shift) = offbeat(grid) else {
        return;
    };
    for (phrase, led_in) in section.phrases.iter().zip(&crossings.led_in) {
        let start = section.start + grid.bar_ticks() * phrase.start_bar as i64;
        if crossings.anticipates {
            for position in (1..phrase.bars).step_by(2) {
                let at = start + grid.bar_ticks() * position as i64;
                anticipate_downbeat(section, at, shift, notes);
            }
        }
        if *led_in {
            pick_up(settings, grid, section, part, start, shift, notes);
        }
    }
}

/// Pull the note on the downbeat at `at` an offbeat ahead of it, making room in the note before.
///
/// Refused when the bar does not enter on its downbeat, when the entry is shorter than the
/// offbeat it would be moved by, when the note before it would be left shorter than that, and
/// when no chord sounds where the note would move to — a stretch somebody cleared is silent,
/// and an anticipation is not a way into it.
fn anticipate_downbeat(
    section: &SectionPlan,
    at: Ticks,
    shift: Ticks,
    notes: &mut [Draft],
) -> bool {
    let index = notes.partition_point(|note| note.start < at);
    let (before, from) = notes.split_at_mut(index);
    let (Some(previous), Some(entry)) = (before.last_mut(), from.first_mut()) else {
        return false;
    };
    let room = at - shift;
    if entry.start != at
        || entry.length < shift
        || previous.start + shift > room
        || section.chord_at(room - section.start).is_none()
    {
        return false;
    }
    previous.length = previous.length.min(room - previous.start);
    entry.start = room;
    entry.length += shift;
    true
}

/// Lead into the phrase starting at `start` with one note in the breath before it.
///
/// The note is a scale step from the downbeat it leads to, in the scale of the chord it
/// sounds over, approached from the side the phrase before left off on — so the line passes
/// through it rather than turning round for it. An offbeat of silence is kept in front of it:
/// refused when the phrase before would have to be cut into to leave one, when the new phrase
/// does not enter on its downbeat, and when the step would leave the part's range.
fn pick_up(
    settings: &ScoreSettings,
    grid: Grid,
    section: &SectionPlan,
    part: &PartSpec,
    start: Ticks,
    shift: Ticks,
    notes: &mut Vec<Draft>,
) -> bool {
    let index = notes.partition_point(|note| note.start < start);
    let (Some(previous), Some(entry)) = (index.checked_sub(1).map(|i| notes[i]), notes.get(index))
    else {
        return false;
    };
    let entry = *entry;
    let at = start - shift;
    if entry.start != start || previous.start + previous.length > at - shift {
        return false;
    }
    let Some(event) = section.chord_at(at - section.start) else {
        return false;
    };
    let scale = ChordScale::new(event.key, event.chord);
    let (low, high) = part.range();
    let target = i32::from(entry.pitch);
    // From below when the line is rising into the phrase or standing still, from above when
    // it is falling into it; the other side when that one is out of range.
    let side = if i32::from(previous.pitch) > target {
        1
    } else {
        -1
    };
    let Some(pitch) = [side, -side]
        .into_iter()
        .map(|step| scale_shift(&scale, target, step))
        .find(|pitch| (low..=high).contains(pitch) && *pitch != target)
    else {
        return false;
    };
    let within = (at - section.start)
        .raw()
        .rem_euclid(grid.bar_ticks().raw().max(1));
    let step = grid.step_of(Ticks(within));
    notes.insert(
        index,
        Draft {
            section: entry.section,
            pitch: pitch.clamp(0, 127) as u8,
            velocity: (velocity(grid.weight(step), section.intensity, settings.dynamics)
                * phrase_shape(grid, section, at - section.start, settings.dynamics))
            .clamp(0.05, 1.0),
            start: at,
            length: shift,
        },
    );
    true
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
    use crate::fixture::Song;

    /// The default stretch, whose first part is the lead.
    fn lead() -> (Song, Frame) {
        let song = Song::default();
        let frame = song.frame();
        (song, frame)
    }

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
        let (_, frame) = lead();
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
        let (_, frame) = lead();
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
    fn an_anticipated_downbeat_is_held_across_the_bar_line() {
        let (_, frame) = lead();
        let section = &frame.sections[0];
        let half = Ticks::from_beats(0.5);
        let bar = Ticks::from_beats(4.0);
        let mut notes = [note(2.0, 2.0, 67), note(4.0, 1.0, 69), note(5.0, 1.0, 71)];
        assert!(anticipate_downbeat(section, bar, half, &mut notes));
        assert_eq!(
            notes,
            [note(2.0, 1.5, 67), note(3.5, 1.5, 69), note(5.0, 1.0, 71)]
        );
        // Where the bar before already left the room, nothing in it is shortened.
        let mut rested = [note(2.0, 1.0, 67), note(4.0, 1.0, 69)];
        assert!(anticipate_downbeat(section, bar, half, &mut rested));
        assert_eq!(rested, [note(2.0, 1.0, 67), note(3.5, 1.5, 69)]);
    }

    #[test]
    fn an_anticipation_needs_a_downbeat_to_move_and_a_note_it_can_shorten() {
        let (_, mut frame) = lead();
        let section = frame.sections[0].clone();
        let half = Ticks::from_beats(0.5);
        let bar = Ticks::from_beats(4.0);
        for mut notes in [
            // The bar enters late: there is no downbeat to pull ahead.
            [note(2.0, 2.0, 67), note(4.5, 1.0, 69)],
            // The note before would be left with nothing.
            [note(3.5, 0.5, 67), note(4.0, 1.0, 69)],
            // The entry is shorter than the offbeat it would be moved by.
            [note(2.0, 2.0, 67), note(4.0, 0.25, 69)],
        ] {
            let before = notes;
            assert!(!anticipate_downbeat(&section, bar, half, &mut notes));
            assert_eq!(notes, before);
        }
        // And the first bar of all has no bar before it.
        let mut first = [note(0.0, 1.0, 67)];
        assert!(!anticipate_downbeat(
            &section,
            Ticks::ZERO,
            half,
            &mut first
        ));
        // A stretch with no chord under it is silent, and nothing is moved into it.
        let cleared = &mut frame.sections[0];
        cleared.events[0].length = Ticks::from_beats(3.0);
        let mut notes = [note(1.0, 2.0, 67), note(4.0, 1.0, 69)];
        let before = notes;
        assert!(!anticipate_downbeat(cleared, bar, half, &mut notes));
        assert_eq!(notes, before);
    }

    #[test]
    fn a_pickup_steps_into_the_downbeat_from_the_side_the_line_came_from() {
        let (spec, frame) = lead();
        let section = &frame.sections[0];
        let half = Ticks::from_beats(0.5);
        let bar = Ticks::from_beats(4.0);
        let led = |previous: Draft, entry: Draft| {
            let mut notes = vec![previous, entry];
            let added = pick_up(
                &spec.settings,
                frame.grid,
                section,
                &spec.parts[0],
                bar,
                half,
                &mut notes,
            );
            (added, notes)
        };
        // Rising into G over the bar of C before it: the step below, on the last offbeat.
        let (added, notes) = led(note(0.0, 2.0, 64), note(4.0, 1.0, 67));
        assert!(added);
        assert_eq!(notes.len(), 3);
        assert_eq!(
            (notes[1].start, notes[1].length, notes[1].pitch),
            (Ticks::from_beats(3.5), half, 65)
        );
        assert!(notes[1].velocity > 0.05 && notes[1].velocity <= notes[2].velocity);
        assert_eq!(
            (notes[0], notes[2]),
            (note(0.0, 2.0, 64), note(4.0, 1.0, 67))
        );
        // Falling into it: the step above.
        let (_, notes) = led(note(0.0, 2.0, 72), note(4.0, 1.0, 67));
        assert_eq!(notes[1].pitch, 69);
        // At the floor of the range there is no step below, so it comes from above.
        let (_, notes) = led(note(0.0, 2.0, 60), note(4.0, 1.0, 60));
        assert_eq!(notes[1].pitch, 62);
        // No room: the phrase before is still sounding an offbeat ahead of the pickup.
        let (added, notes) = led(note(0.0, 3.5, 64), note(4.0, 1.0, 67));
        assert!(!added);
        assert_eq!(notes.len(), 2);
        // No downbeat: a phrase that enters late has its own way in.
        let (added, _) = led(note(0.0, 2.0, 64), note(4.5, 1.0, 67));
        assert!(!added);
    }

    #[test]
    fn crossings_are_planned_per_section_and_never_into_the_first_phrase() {
        let (spec, template) = lead();
        let mut frame = template.clone();
        let section = &mut frame.sections[0];
        section.bars = 16;
        section.length = frame.grid.bar_ticks() * 16;
        section.phrases = crate::phrasing::plan_phrases(16, None);
        let section = frame.sections[0].clone();
        let (mut anticipating, mut led_in) = (0, 0);
        for seed in 0..64 {
            frame.seed = seed;
            let plan = Crossings::plan(&spec.settings, &frame, &section, &spec.parts[0]);
            assert_eq!(plan.led_in.len(), 4);
            assert!(!plan.led_in[0], "seed {seed} leads into the first phrase");
            assert_eq!(
                plan,
                Crossings::plan(&spec.settings, &frame, &section, &spec.parts[0])
            );
            // The phrase holding a bar makes room when the one after it is led into.
            for (index, phrase) in section.phrases.iter().enumerate() {
                let next = plan.led_in.get(index + 1).copied().unwrap_or(false);
                assert_eq!(plan.follow(&section, phrase.start_bar), next);
                assert_eq!(plan.follow(&section, phrase.end_bar() - 1), next);
            }
            anticipating += usize::from(plan.anticipates);
            led_in += plan.led_in.iter().filter(|led| **led).count();
        }
        assert!((8..=56).contains(&anticipating), "{anticipating} of 64");
        assert!((48..=132).contains(&led_in), "{led_in} of 192 phrases");

        // A written rhythm and a sustained palette cross nothing.
        let mut written = spec.parts[0].clone();
        written.rhythm = crate::rhythm::Pattern::parse("x...x...x...x...");
        assert_eq!(
            Crossings::plan(&spec.settings, &frame, &section, &written),
            Crossings::default()
        );
        let mut sustained = spec.settings.clone();
        sustained.style = Some(PerformanceStyle::Ambient);
        assert_eq!(
            Crossings::plan(&sustained, &frame, &section, &spec.parts[0]),
            Crossings::default()
        );
    }

    #[test]
    fn pickups_and_existing_late_holds_keep_their_timing() {
        let (_, frame) = lead();
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
        let (_, mut frame) = lead();
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
        let (spec, frame) = lead();
        let mut notes: Vec<_> = (0..16)
            .map(|at| note(f64::from(at), 1.0, 60 + (at % 7) as u8))
            .collect();
        let before = notes.clone();
        connect(
            &spec.settings.clone(),
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
        let (spec, frame) = lead();
        let original: Vec<_> = (0..16).map(|at| note(f64::from(at), 1.0, 67)).collect();
        for style in [PerformanceStyle::Ambient, PerformanceStyle::Orchestral] {
            let mut settings = spec.settings.clone();
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
            &spec.settings.clone(),
            &frame,
            &frame.sections[0],
            &part,
            &mut notes,
        );
        assert_eq!(notes, original);
    }

    #[test]
    fn short_meters_do_not_move_a_late_hold_into_the_middle() {
        let (_, frame) = lead();
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
        let (_, mut frame) = lead();
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

        let (spec, template) = lead();
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
                    connect(&spec.settings.clone(), &frame, &section, &part, &mut notes);
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
