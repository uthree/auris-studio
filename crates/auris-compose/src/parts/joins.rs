//! The hits that mark where one section becomes another.
//!
//! Every other writer reads a groove or a chord and fills a bar with it. This one reads the
//! *form*: it is handed a section and asked whether arriving there is worth striking something
//! for. That is a different question from any of the others, and it is why a crash could not be
//! got at by adding a voice to a groove — a groove is a bar-long loop, and there is nothing in a
//! bar-long loop that knows a chorus has just started.
//!
//! The riser, the impact and the reverse cymbal are the same question asked at the same joins, so
//! they belong here when they arrive rather than in a file each.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::DrumVoice;
use crate::spec::PartSpec;

use super::writer::{dynamic, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// How hard a piece has to open before it opens on a cymbal.
///
/// A crash is an arrival, and the first bar of a song has not arrived from anywhere. Most pieces
/// therefore start without one — an intro at 0.30 that opened on a cymbal would be announcing
/// something that has not happened yet. What this leaves room for is the piece that starts
/// *already going*, straight into a chorus, which is a real way to begin a record and does take
/// the cymbal.
const OPENING_INTENSITY: f32 = 0.7;

/// How long a cymbal's note is written for.
///
/// Longer than the 120 ticks the rest of the kit gets, and for the same reason that number is
/// short: a one-shot ignores its note-off, so the length is there to be *read*. An eighth is what
/// a cymbal looks like in a piano roll — long enough to be seen among the sixteenths of a groove,
/// short enough not to cover the bar it starts.
const CYMBAL_TICKS: i64 = 480;

/// Whether arriving at a section is worth striking a cymbal for.
///
/// Two claims, and the second is the one that keeps the piece from sounding like a fairground.
///
/// **Arriving at something at least as strong is an arrival.** The intro gives way to the verse
/// and the verse to the chorus, and each of those joins takes a cymbal, because each is a step up
/// or a step across.
///
/// **Coming back down is not.** The verse after a chorus is the one join in a pop form that a
/// drummer almost never crashes: the energy is dropping, and marking that with the loudest thing
/// in the kit says the opposite of what the arrangement is doing. Reading the section *before*
/// rather than the section's own intensity is the whole of how that is told apart — 0.55 is a
/// verse worth crashing into out of an intro and not worth crashing into out of a chorus, and no
/// property of the verse alone can say which of the two it is.
fn marks_an_arrival(section: &SectionPlan, before: Option<&SectionPlan>) -> bool {
    match before {
        Some(before) => section.intensity >= before.intensity,
        None => section.intensity >= OPENING_INTENSITY,
    }
}

/// One cymbal, struck where the form arrives somewhere.
pub(super) fn joins(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let voice = part.role.drum_voice().unwrap_or(DrumVoice::Crash);
    let pitch = part.drum_note().unwrap_or_else(|| voice.pitch());
    let grid = part_grid(frame, part);

    // A part that says where it lands has answered the only question this writer asks, so it is
    // played as written — the same trade the groove gets, for the same reason. What it does not
    // get is the thinning and the ghosting a groove gets: both of those are what to do with a
    // *suggestion*, and neither has anything to say about a cymbal in any case.
    if let Some(pattern) = &part.rhythm {
        let mut notes = Vec::new();
        for bar in 0..section.bars {
            let bar_start = grid.bar_ticks() * bar as i64;
            for step in 0..grid.steps_per_bar() {
                let Some(accent) = pattern.at(step) else {
                    continue;
                };
                let at = bar_start + grid.tick_of(step);
                notes.push(Draft {
                    section: index,
                    pitch,
                    velocity: (velocity(grid.weight(step), section.intensity, settings.dynamics)
                        * dynamic(accent.scale(), settings.dynamics)
                        * phrase_shape(grid, section, at, settings.dynamics))
                    .clamp(0.08, 1.0),
                    start: section.start + at,
                    length: Ticks(CYMBAL_TICKS),
                });
            }
        }
        return notes;
    }

    if !marks_an_arrival(
        section,
        index.checked_sub(1).and_then(|i| frame.sections.get(i)),
    ) {
        return Vec::new();
    }

    vec![Draft {
        section: index,
        pitch,
        // The downbeat's own weight, so the cymbal sits in the same dynamic world as the kit it
        // is part of and flattens with it when `dynamics` is turned down.
        //
        // No phrase shape. Every other part leans up across a section, which means its first bar
        // is its quietest — and the first bar is the only place this writer puts anything. A
        // cymbal marking an arrival at 88 per cent of the level of the bars that follow it would
        // be the arrival being the softest moment of the section it opens.
        velocity: velocity(grid.weight(0), section.intensity, settings.dynamics).clamp(0.08, 1.0),
        start: section.start,
        length: Ticks(CYMBAL_TICKS),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parts::fixture::{draft, part};

    /// A plan with one section at each of the given intensities, to ask `marks_an_arrival` about.
    fn run(intensities: &[f32]) -> Vec<bool> {
        let sections: Vec<SectionPlan> = intensities
            .iter()
            .map(|intensity| SectionPlan {
                name: "s".to_string(),
                instance: 1,
                start: Ticks::ZERO,
                length: Ticks::ZERO,
                bars: 4,
                key: crate::theory::key::Key::parse("C major").unwrap(),
                tempo: 120.0,
                intensity: *intensity,
                events: Vec::new(),
                skeleton: Vec::new(),
                parts: Vec::new(),
                tweaks: Default::default(),
            })
            .collect();
        (0..sections.len())
            .map(|index| {
                marks_an_arrival(
                    &sections[index],
                    index.checked_sub(1).map(|before| &sections[before]),
                )
            })
            .collect()
    }

    #[test]
    fn a_pop_form_crashes_where_it_arrives_and_not_where_it_comes_down() {
        // The shipped default form, at the intensities `SectionSpec::named` gives it. Three
        // cymbals in six sections: into the verse, into each chorus, and nowhere else.
        //
        //          intro verse chorus verse chorus outro
        assert_eq!(
            run(&[0.30, 0.55, 0.90, 0.55, 0.90, 0.25]),
            [false, true, true, false, true, false]
        );
    }

    #[test]
    fn a_quiet_opening_does_not_start_on_a_cymbal() {
        // Nothing has been arrived at yet, so the level of the opening is all there is to go on.
        assert_eq!(run(&[0.30]), [false]);
        assert_eq!(run(&[0.95]), [true], "a piece can start already going");
    }

    #[test]
    fn a_section_repeated_at_its_own_level_is_still_an_arrival() {
        // Verse into verse: nothing is dropping, and the top of the form is where a drummer
        // crashes whether or not the music got louder.
        assert_eq!(run(&[0.55, 0.55]), [false, true]);
    }

    #[test]
    fn the_cymbal_lands_on_the_downbeat_of_the_section_it_opens() {
        let (_, frame, parts) = draft(
            r#"
            form = "intro chorus"
            chords = "@axis"
            humanize = 0
            [section.intro]
            bars = 4
            intensity = 0.3
            [section.chorus]
            bars = 4
            intensity = 0.9
            [[part]]
            name = "crash"
            role = "crash"
            "#,
        );
        let crash = part(&parts, "crash");
        let starts: Vec<i64> = crash.notes.iter().map(|note| note.start.raw()).collect();
        assert_eq!(
            starts,
            [frame.sections[1].start.raw()],
            "one cymbal, on the chorus downbeat"
        );
        assert!(crash.notes.iter().all(|note| note.pitch == 49));
    }

    #[test]
    fn a_written_rhythm_is_played_where_it_says_rather_than_at_the_joins() {
        // The intro would take no cymbal at all on the form rule. A part that says where it lands
        // has answered the question this writer asks, and silently ignoring the line would be the
        // format dropping an instruction.
        let (_, frame, parts) = draft(
            r#"
            form = "intro"
            chords = "@axis"
            humanize = 0
            [section.intro]
            bars = 2
            intensity = 0.3
            [[part]]
            name = "crash"
            role = "crash"
            rhythm = "x ~ ~ ~ ~ ~ ~ ~ x ~ ~ ~ ~ ~ ~ ~"
            "#,
        );
        let crash = part(&parts, "crash");
        let bar = frame.grid.bar_ticks().raw();
        let starts: Vec<i64> = crash.notes.iter().map(|note| note.start.raw()).collect();
        assert_eq!(starts, [0, bar / 2, bar, bar + bar / 2]);
    }
}
