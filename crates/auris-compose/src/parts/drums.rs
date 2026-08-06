//! One drum voice, and the fill that runs a section into whatever follows it.
//!
//! Apart from the pitched writers because almost nothing they read applies here. A drum has no
//! range, no scale and no voicing; its density thins or thickens a groove somebody already wrote
//! instead of choosing notes; and the length it writes is only there to make the piano roll
//! readable, because a one-shot ignores its note-off. The fill is here rather than with the form
//! because it is the snare that plays it.

use auris_core::time::Ticks;

use crate::frame::{Frame, SectionPlan};
use crate::rhythm::{Accent, DrumVoice};
use crate::spec::PartSpec;

use super::writer::{bar_stream, dynamic, part_grid, phrase_shape, velocity};
use super::{Draft, ScoreSettings};

/// One drum voice.
pub(super) fn drums(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
) -> Vec<Draft> {
    let voice = part.role.drum_voice().unwrap_or(DrumVoice::ClosedHat);
    // What the part strikes, which is General MIDI unless it says otherwise. A SoundFont kit that
    // does not follow GM comes out silent or playing a cowbell without this.
    let pitch = part.drum_note().unwrap_or_else(|| voice.pitch());
    // A rhythm the user wrote is played as written. Only the groove's own pattern is thinned,
    // because that is the composer's suggestion rather than an instruction.
    let written = part.rhythm.is_some();
    let pattern = part
        .rhythm
        .clone()
        .unwrap_or_else(|| crate::frame::groove_pattern(&settings.groove, voice));
    let grid = part_grid(frame, part);
    let mut notes = Vec::new();
    // How hard the drummer is leaning on the groove. The middle of the dial plays it as written
    // — everything below thins it, everything above fills it in — so that a kit nobody has
    // touched plays the pattern somebody wrote rather than a version of it. *Which* groove is
    // still the groove: this is not a second way to spell that.
    //
    // Read straight off the dial rather than through `density`, which folds the section's
    // intensity in. The survival roll below already weighs the intensity, and counting it twice
    // would thin a quiet section twice as fast as its own number says — and would put the
    // neutral position somewhere nobody could find.
    let dialled = part.density.unwrap_or(0.5).clamp(0.0, 1.0);
    let leaning = 0.5 + dialled;
    // Above the middle, the steps the groove left empty start taking ghost notes. That is how a
    // drummer gets busier without playing something else — and it is why they are ghosts and why
    // they land on the weak steps only. A filled-in step arriving at full weight would not be a
    // busier groove, it would be a different one.
    let ghosting = (dialled - 0.5).max(0.0) * 2.0;

    for bar in 0..section.bars {
        let mut rng = bar_stream(settings, frame, part, section, "drums", bar);
        // The ghosts draw from a stream of their own. They exist only above the dial's middle,
        // and a draw that appears the moment the dial crosses it would shift every later
        // survival roll in the bar — nudging density from 0.50 to 0.51 rescrambled which hits
        // of the groove survive instead of only adding ghosts. One decision, one stream.
        let mut haunt = bar_stream(settings, frame, part, section, "ghosts", bar);
        let bar_start = grid.bar_ticks() * bar as i64;
        // Which steps ended up carrying a hit, so a fill can go round them rather than double
        // them: the pattern says where a hit belongs and thinning may already have taken it away.
        let mut played = vec![false; grid.steps_per_bar()];
        for (step, sounded) in played.iter_mut().enumerate() {
            let weight = grid.weight(step);
            let accent = match pattern.at(step) {
                Some(accent) => {
                    // A quiet section thins the pattern out rather than playing it softly, which
                    // is what a drummer does. The downbeat is never thinned, or the bar loses its
                    // footing.
                    //
                    // What thins a hit is how weak its step is, and how quietly the section is
                    // being played. A *beat* survives outright at the middle of the dial: the
                    // arithmetic here used to drop one in three of them at the default settings,
                    // which is not a drummer playing quietly, it is a drummer missing — and it
                    // is why the kit came out too sparse to hold a song up.
                    let strength = match weight {
                        0 => 0.72,
                        1 => 0.90,
                        _ => 1.0,
                    };
                    let survives = strength * (0.70 + 0.30 * section.intensity) * leaning;
                    if !written && weight < 4 && !rng.chance(survives.clamp(0.0, 1.0)) {
                        continue;
                    }
                    accent
                }
                // A rhythm somebody wrote is played as written, so nothing is added to one
                // either: thinning and filling are both what to do with a suggestion.
                None if written || weight > 1 || ghosting <= 0.0 => continue,
                None if !haunt.chance(ghosting * 0.45) => continue,
                None => Accent::Ghost,
            };
            let at = bar_start + grid.tick_of(step);
            *sounded = true;
            notes.push(Draft {
                section: index,
                pitch,
                velocity: (velocity(weight, section.intensity, settings.dynamics)
                    * dynamic(accent.scale(), settings.dynamics)
                    * phrase_shape(grid, section, at, settings.dynamics))
                .clamp(0.08, 1.0),
                start: section.start + at,
                // A one-shot drum ignores its note-off, so the length is only there to make the
                // piano roll readable.
                length: Ticks(120),
            });
        }
        // A fill is a departure from a groove, so there has to be a groove to depart from. A
        // name nobody recognises leaves every voice a bar of rests, and running a fill over that
        // would be the kit inventing a part out of a typo.
        if pattern.hits() > 0 {
            fill(
                settings, frame, section, index, part, voice, bar, &played, &mut notes,
            );
        }
    }
    notes
}

/// Runs the snare into whatever follows the section.
///
/// A section that simply stops and is replaced sounds like an edit rather than like an arrival:
/// the join is the one moment a listener is certain to notice, and nothing marked it. Only the
/// last bar of a section gets one, and only the snare plays it — the other voices keep the groove
/// underneath so the fill has something to be a departure from.
///
/// A part with a written rhythm is left alone, on the same principle as thinning: an instruction
/// is not a suggestion.
#[allow(clippy::too_many_arguments)]
fn fill(
    settings: &ScoreSettings,
    frame: &Frame,
    section: &SectionPlan,
    index: usize,
    part: &PartSpec,
    voice: DrumVoice,
    bar: usize,
    played: &[bool],
    notes: &mut Vec<Draft>,
) {
    let last_bar = bar + 1 == section.bars;
    // The last section of a piece has nothing to lead into and plays the groove to the end.
    let leads_somewhere = index + 1 < frame.sections.len() || frame.joins_on;
    if part.rhythm.is_some() || voice != DrumVoice::Snare || !last_bar || !leads_somewhere {
        return;
    }

    let grid = part_grid(frame, part);
    let steps = grid.steps_per_bar();
    let per_beat = grid.steps_per_beat as usize;
    // How much of the bar runs, from none to two beats. The section's intensity still leans on
    // it, so a quiet section fills shorter than a loud one at the same setting — the dial says
    // how much of a fill this piece wants, not how much this one bar gets.
    let wanted = settings.fill.clamp(0.0, 1.0) * (0.6 + 0.4 * section.intensity);
    let beats = (wanted * 2.0).round() as usize;
    if beats == 0 {
        return;
    }
    let from = steps.saturating_sub(beats * per_beat).max(1);
    let bar_start = grid.bar_ticks() * bar as i64;

    for step in from..steps {
        if played.get(step).copied().unwrap_or(false) {
            continue;
        }
        // Rising into the downbeat that follows, which is what makes it lead somewhere — and the
        // rise is a dynamic like any other, so it flattens with the rest of them rather than
        // being the one crescendo left standing in a part played at one level on purpose.
        let through = (step - from) as f32 / (steps - from).max(1) as f32;
        let mean = 0.70;
        let rise = mean + (0.45 + 0.5 * through - mean) * settings.dynamics.clamp(0.0, 1.0);
        notes.push(Draft {
            section: index,
            // The same note the groove is being played on, or the fill would run on the snare a
            // General MIDI kit has rather than the one this instrument actually carries.
            pitch: part.drum_note().unwrap_or_else(|| voice.pitch()),
            velocity: rise.clamp(0.08, 1.0),
            start: section.start + bar_start + grid.tick_of(step),
            length: Ticks(120),
        });
    }
}

#[cfg(test)]
mod tests {
    use crate::parts::fixture::{BASE, draft, part, section_notes};

    #[test]
    fn drums_play_their_general_midi_pitches() {
        let (_, _, parts) = draft(BASE);
        for (name, pitch) in [("kick", 36), ("snare", 38), ("hat", 42)] {
            let drum = part(&parts, name);
            assert!(
                drum.notes.iter().all(|note| note.pitch == pitch),
                "`{name}` played something other than {pitch}"
            );
        }
    }

    #[test]
    fn a_written_rhythm_survives_a_quiet_section() {
        // Thinning is a suggestion about the groove, not licence to ignore an instruction.
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            humanize = 0
            [section.verse]
            bars = 1
            intensity = 0.05
            [[part]]
            name = "kick"
            rhythm = "x ~ x ~ x ~ x ~ x ~ x ~ x ~ x ~"
            "#,
        );
        let steps: Vec<usize> = part(&parts, "kick")
            .notes
            .iter()
            .map(|note| frame.grid.step_of(note.start))
            .collect();
        assert_eq!(steps, vec![0, 2, 4, 6, 8, 10, 12, 14]);
    }

    #[test]
    fn a_written_rhythm_is_played_as_written() {
        let (_, frame, parts) = draft(
            r#"
            form = "verse"
            humanize = 0
            [section.verse]
            bars = 1
            [[part]]
            name = "kick"
            rhythm = "x ~ ~ ~ x ~ ~ ~ x ~ ~ ~ x ~ ~ ~"
            "#,
        );
        let kick = part(&parts, "kick");
        let steps: Vec<usize> = kick
            .notes
            .iter()
            .map(|note| frame.grid.step_of(note.start))
            .collect();
        assert_eq!(steps, vec![0, 4, 8, 12]);
    }

    #[test]
    fn a_louder_section_plays_more_drum_hits() {
        let quiet = draft(&BASE.replace("bars = 4", "bars = 4\nintensity = 0.1")).2;
        let loud = draft(&BASE.replace("bars = 4", "bars = 4\nintensity = 1.0")).2;
        assert!(
            part(&loud, "hat").notes.len() > part(&quiet, "hat").notes.len(),
            "intensity did not change how much the drummer plays"
        );
    }

    #[test]
    fn a_section_runs_a_fill_into_the_one_that_follows() {
        // A section that stopped and was replaced sounded like an edit rather than an arrival.
        // The last section of a piece has nothing to lead into, so it keeps the groove instead.
        let (_, frame, parts) = draft(
            r#"
                form = "verse verse"
                chords = "@axis"
                humanize = 0
                variation = 0
                [section.verse]
                bars = 4
                intensity = 0.8
                "#,
        );
        let snare = part(&parts, "snare");
        let bar = frame.grid.bar_ticks();
        let last_bar_hits = |section: usize| -> usize {
            let plan = &frame.sections[section];
            snare
                .notes
                .iter()
                .filter(|note| {
                    note.section == section && note.start >= plan.start + plan.length - bar
                })
                .count()
        };
        assert!(
            last_bar_hits(0) > last_bar_hits(1),
            "the first verse ran {} hits into the second's {}",
            last_bar_hits(0),
            last_bar_hits(1)
        );
    }

    #[test]
    fn nudging_density_past_the_middle_only_adds_ghosts() {
        // Crossing 0.5 used to insert a ghost draw before every later survival roll in the
        // bar, so the dial's smallest movement rescrambled which hits of the groove survive
        // instead of only thickening the playing. With the ghosts on a stream of their own,
        // everything the kit plays at the middle it still plays above it — the ghosts arrive
        // on top.
        let kit = |density: f32| {
            let text = format!(
                r#"
                    form = "verse"
                    chords = "@axis"
                    humanize = 0
                    swing = 50
                    [section.verse]
                    bars = 4
                    [[part]]
                    name = "kick"
                    density = {density}
                    "#
            );
            let (_, frame, parts) = draft(&text);
            section_notes(&frame, part(&parts, "kick"), 0)
                .into_iter()
                .map(|(start, pitch, ..)| (start, pitch))
                .collect::<Vec<_>>()
        };
        let middle = kit(0.5);
        let above = kit(0.9);
        for note in &middle {
            assert!(
                above.contains(note),
                "a groove hit at {note:?} was lost by asking for *more*"
            );
        }
        assert!(
            above.len() > middle.len(),
            "nothing was added above the middle"
        );
    }
}
